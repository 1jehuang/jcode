#[test]
fn session_picker_resume_action_keeps_overlay_open() {
    let mut app = create_test_app();
    app.session_picker_mode = SessionPickerMode::CatchUp;
    app.session_picker_overlay = Some(RefCell::new(
        crate::tui::session_picker::SessionPicker::new(vec![
            crate::tui::session_picker::SessionInfo {
                id: "session_keep_open".to_string(),
                parent_id: None,
                short_name: "keep-open".to_string(),
                icon: "k".to_string(),
                title: "Keep Open".to_string(),
                message_count: 1,
                user_message_count: 1,
                assistant_message_count: 0,
                created_at: chrono::Utc::now(),
                last_message_time: chrono::Utc::now(),
                last_active_at: None,
                working_dir: None,
                model: None,
                provider_key: None,
                is_canary: false,
                is_debug: false,
                saved: false,
                save_label: None,
                status: crate::session::SessionStatus::Closed,
                needs_catchup: false,
                estimated_tokens: 0,
                first_user_prompt: None,
                messages_preview: Vec::new(),
                search_index: "keep-open keep open".to_string(),
                server_name: None,
                server_icon: None,
                source: crate::tui::session_picker::SessionSource::Jcode,
                resume_target: crate::tui::session_picker::ResumeTarget::JcodeSession {
                    session_id: "session_keep_open".to_string(),
                },
                external_path: None,
            },
        ]),
    ));

    app.handle_session_picker_key(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::empty(),
    )
    .expect("session picker enter should succeed");

    assert!(app.session_picker_overlay.is_some());
}

#[test]
fn session_picker_enter_queues_current_terminal_resume_and_closes_overlay() {
    let mut app = create_test_app();
    app.session_picker_mode = SessionPickerMode::Resume;
    app.session_picker_overlay = Some(RefCell::new(
        crate::tui::session_picker::SessionPicker::new(vec![
            crate::tui::session_picker::SessionInfo {
                id: "session_here_123".to_string(),
                parent_id: None,
                short_name: "here".to_string(),
                icon: "h".to_string(),
                title: "Here".to_string(),
                message_count: 1,
                user_message_count: 1,
                assistant_message_count: 0,
                created_at: chrono::Utc::now(),
                last_message_time: chrono::Utc::now(),
                last_active_at: None,
                working_dir: None,
                model: None,
                provider_key: None,
                is_canary: false,
                is_debug: false,
                saved: false,
                save_label: None,
                status: crate::session::SessionStatus::Closed,
                needs_catchup: false,
                estimated_tokens: 0,
                first_user_prompt: None,
                messages_preview: Vec::new(),
                search_index: "here".to_string(),
                server_name: None,
                server_icon: None,
                source: crate::tui::session_picker::SessionSource::Jcode,
                resume_target: crate::tui::session_picker::ResumeTarget::JcodeSession {
                    session_id: "session_here_123".to_string(),
                },
                external_path: None,
            },
        ]),
    ));

    app.handle_session_picker_key(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::empty(),
    )
    .expect("session picker enter should succeed");

    assert!(app.session_picker_overlay.is_none());
    assert_eq!(
        app.workspace_client.take_pending_resume_session().as_deref(),
        Some("session_here_123")
    );
}

#[test]
fn slash_resume_opens_session_picker_overlay_locally() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    let _guard = runtime.enter();
    let mut app = create_test_app();

    app.input = "/resume".to_string();
    app.submit_input();

    assert!(app.session_picker_overlay.is_some());
    assert_eq!(app.session_picker_mode, SessionPickerMode::Resume);
    assert!(app.pending_session_picker_load.is_some());
    assert!(app.input.is_empty());
}

#[test]
fn slash_command_submit_retains_pending_images() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    let _guard = runtime.enter();
    let mut app = create_test_app();

    app.pending_images
        .push(("image/png".to_string(), "aGVsbG8=".to_string()));
    app.input = "/help".to_string();
    app.submit_input();

    // Slash commands are handled locally and must not consume attached images;
    // the images stay pending and go out with the next real prompt submission.
    assert_eq!(app.pending_images.len(), 1);
    assert_eq!(app.pending_images[0].0, "image/png");
    assert!(app.input.is_empty());
}

#[test]
fn slash_sessions_alias_opens_session_picker_overlay_locally() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    let _guard = runtime.enter();
    let mut app = create_test_app();

    app.input = "/sessions".to_string();
    app.submit_input();

    assert!(app.session_picker_overlay.is_some());
    assert_eq!(app.session_picker_mode, SessionPickerMode::Resume);
    assert!(app.pending_session_picker_load.is_some());
    assert!(app.input.is_empty());
}

#[test]
fn slash_session_alias_opens_session_picker_overlay_locally() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    let _guard = runtime.enter();
    let mut app = create_test_app();

    app.input = "/session".to_string();
    app.submit_input();

    assert!(app.session_picker_overlay.is_some());
    assert_eq!(app.session_picker_mode, SessionPickerMode::Resume);
    assert!(app.pending_session_picker_load.is_some());
    assert!(app.input.is_empty());
}

#[test]
fn slash_active_opens_active_sessions_picker_locally() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    let _guard = runtime.enter();
    let mut app = create_test_app();

    app.input = "/active".to_string();
    app.submit_input();

    assert!(app.session_picker_overlay.is_some());
    assert_eq!(app.session_picker_mode, SessionPickerMode::ActiveSessions);
    assert!(app.pending_session_picker_load.is_some());
    assert!(app.input.is_empty());
}

#[test]
fn left_arrow_on_empty_input_is_a_noop_unless_opted_in() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    let _guard = runtime.enter();
    let mut app = create_test_app();

    // Default config: the active sessions manager gesture is opt-in, so Left
    // on an empty input must not open any overlay.
    assert!(!app.maybe_open_active_sessions_on_left());
    assert!(app.session_picker_overlay.is_none());

    // With text in the input the gesture never fires regardless of config.
    app.input = "hello".to_string();
    app.cursor_pos = 0;
    assert!(!app.maybe_open_active_sessions_on_left());
    assert!(app.session_picker_overlay.is_none());
}

#[test]
fn test_resize_redraw_is_debounced() {
    let mut app = create_test_app();

    assert!(app.should_redraw_after_resize());
    assert!(!app.should_redraw_after_resize());
    assert!(app.resize_redraw_pending);

    app.last_resize_redraw = Some(Instant::now() - Duration::from_millis(40));
    assert!(app.flush_pending_resize_redraw());
    assert!(!app.resize_redraw_pending);
    assert!(!app.flush_pending_resize_redraw());
}

#[test]
fn test_help_topic_shows_command_details() {
    let mut app = create_test_app();
    app.input = "/help compact".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("/compact"));
    assert!(msg.content.contains("background"));
    assert!(msg.content.contains("/compact mode"));
}

#[test]
fn test_help_topic_shows_provider_test_coverage_command_details() {
    let mut app = create_test_app();
    app.input = "/help provider-test-coverage".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("/provider-test-coverage"));
    assert!(msg.content.contains("live verification evidence"));
    assert!(msg.content.contains("readiness gaps"));
}

#[test]
fn test_help_topic_shows_log_command_details() {
    let mut app = create_test_app();
    app.input = "/help log".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("/log mark [note]"));
    assert!(msg.content.contains("JCODE_LOG_MARK"));
}

#[test]
fn slash_log_mark_reports_marker_and_note() {
    let mut app = create_test_app();
    app.input = "/log mark before repro".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing log mark response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("Log mark written: logmark-"));
    assert!(msg.content.contains("JCODE_LOG_MARK"));
    assert!(msg.content.contains("Note: before repro"));
}

#[test]
fn slash_log_without_mark_shows_usage() {
    let mut app = create_test_app();
    app.input = "/log".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing log usage response");
    assert_eq!(msg.role, "error");
    assert!(msg.content.contains("Usage: /log mark [note]"));
}

#[test]
fn slash_provider_test_coverage_without_args_shows_cli_style_summary() {
    let mut app = create_test_app();
    app.input = "/provider-test-coverage".to_string();
    app.submit_input();

    assert!(app.model_status_scroll.is_some());
    assert!(
        app.model_status_content
            .starts_with("Live provider/model E2E coverage"),
        "unexpected content: {}",
        app.model_status_content
    );
    assert!(
        app.model_status_content.contains("Coverage:")
            || app
                .model_status_content
                .contains("Status: no verification ledger found"),
        "unexpected content: {}",
        app.model_status_content
    );
}

#[test]
fn slash_provider_test_coverage_with_args_shows_provider_detail() {
    let mut app = create_test_app();
    app.input = "/provider-test-coverage fpt FPT.AI-KIE-v1.7".to_string();
    app.submit_input();

    assert!(app.model_status_scroll.is_some());
    assert!(
        app.model_status_content
            .starts_with("# Provider test coverage")
    );
    assert!(app.model_status_content.contains("Provider: fpt"));
    assert!(
        app.model_status_content
            .contains("Model: FPT.AI-KIE-v1.7")
    );
}

#[test]
fn slash_provider_test_coverage_overlay_scrolls_with_mouse_wheel() {
    let mut app = create_test_app();
    app.input = "/provider-test-coverage".to_string();
    app.submit_input();

    assert_eq!(app.model_status_scroll, Some(0));

    let scroll_only = app.handle_mouse_event(crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::ScrollDown,
        column: 10,
        row: 10,
        modifiers: crossterm::event::KeyModifiers::empty(),
    });
    assert!(scroll_only);
    assert!(app.model_status_scroll.unwrap_or(0) > 0);

    let before = app.model_status_scroll.unwrap_or(0);
    let scroll_only = app.handle_mouse_event(crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::ScrollUp,
        column: 10,
        row: 10,
        modifiers: crossterm::event::KeyModifiers::empty(),
    });
    assert!(scroll_only);
    assert!(app.model_status_scroll.unwrap_or(usize::MAX) < before);
}

#[test]
fn session_picker_preview_wheel_uses_shared_scroll_momentum() {
    use crate::tui::session_picker::{PreviewMessage, SessionInfo, SessionSource};
    // Build a session whose preview overflows a small pane so it can scroll.
    let mut messages = Vec::new();
    for i in 0..40 {
        messages.push(PreviewMessage {
            role: "user".to_string(),
            content: format!("prompt line {i}"),
            tool_calls: Vec::new(),
            tool_data: None,
            timestamp: None,
        });
        messages.push(PreviewMessage {
            role: "assistant".to_string(),
            content: format!("assistant reply {i}"),
            tool_calls: Vec::new(),
            tool_data: None,
            timestamp: None,
        });
    }
    let session = SessionInfo {
        id: "session_scroll".to_string(),
        parent_id: None,
        short_name: "scroll".to_string(),
        icon: "s".to_string(),
        title: "Scroll".to_string(),
        message_count: messages.len(),
        user_message_count: 40,
        assistant_message_count: 40,
        created_at: chrono::Utc::now(),
        last_message_time: chrono::Utc::now(),
        last_active_at: None,
        working_dir: None,
        model: None,
        provider_key: None,
        is_canary: false,
        is_debug: false,
        saved: false,
        save_label: None,
        status: crate::session::SessionStatus::Closed,
        needs_catchup: false,
        estimated_tokens: 0,
        first_user_prompt: Some("prompt line 0".to_string()),
        messages_preview: messages,
        search_index: "scroll".to_string(),
        server_name: None,
        server_icon: None,
        source: SessionSource::Jcode,
        resume_target: crate::tui::session_picker::ResumeTarget::JcodeSession {
            session_id: "session_scroll".to_string(),
        },
        external_path: None,
    };

    let mut picker = crate::tui::session_picker::SessionPicker::new(vec![session]);
    // Render once so the preview pane area + max scroll are populated, and the
    // auto-scroll-to-bottom completes (so a wheel up has room to move). Wheel
    // routing is coordinate-based, so pane focus does not matter here.
    let backend = ratatui::backend::TestBackend::new(120, 20);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| picker.render(frame))
        .expect("render picker");

    let mut app = create_test_app();
    app.session_picker_mode = SessionPickerMode::Resume;
    app.session_picker_overlay = Some(RefCell::new(picker));

    let scroll_before = app
        .session_picker_overlay
        .as_ref()
        .unwrap()
        .borrow()
        .preview_scroll_offset_for_test();
    assert!(
        scroll_before > 0,
        "long preview should auto-scroll to the bottom on first render"
    );

    // A wheel up over the preview pane (right ~60% of width) routes through the
    // shared mouse-scroll momentum (enqueue + drain) instead of an instant jump,
    // and actually moves the preview offset.
    let scroll_only = app.handle_mouse_event(crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::ScrollUp,
        column: 90,
        row: 10,
        modifiers: crossterm::event::KeyModifiers::empty(),
    });
    assert!(
        scroll_only,
        "preview wheel should be classified as scroll-only"
    );
    // Drain any remaining queued momentum so the move completes.
    for _ in 0..32 {
        app.progress_mouse_scroll_animation();
    }
    let scroll_after = app
        .session_picker_overlay
        .as_ref()
        .unwrap()
        .borrow()
        .preview_scroll_offset_for_test();
    assert!(
        scroll_after < scroll_before,
        "wheel up should scroll the preview toward the top (before={scroll_before}, after={scroll_after})"
    );
    assert!(
        !app.has_pending_mouse_scroll_animation(),
        "momentum queue should drain to empty"
    );
}

#[test]
fn test_help_topic_shows_btw_command_details() {
    let mut app = create_test_app();
    app.input = "/help btw".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("/btw <question>"));
    assert!(msg.content.contains("Forks (splits) the session"));
}

#[test]
fn test_help_topic_shows_fork_command_details() {
    let mut app = create_test_app();
    app.input = "/help fork".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("/fork <prompt>"));
    assert!(msg.content.contains("Alias for /fork"));
}

#[test]
fn test_help_topic_shows_git_command_details() {
    let mut app = create_test_app();
    app.input = "/help git".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("/git"));
    assert!(msg.content.contains("git status --short --branch"));
    assert!(msg.content.contains("/git status"));
}

#[test]
fn test_help_topic_shows_commit_command_details() {
    let mut app = create_test_app();
    app.input = "/help commit".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("/commit"));
    assert!(msg.content.contains("logical commits"));
    assert!(msg.content.contains("preserve unrelated work"));
}

#[test]
fn test_commit_command_starts_synthetic_user_turn() {
    let mut app = create_test_app();
    app.input = "/commit".to_string();
    app.submit_input();

    assert!(app.is_processing);
    assert!(app.pending_turn);
    let notice = app
        .display_messages()
        .last()
        .expect("missing launch notice");
    assert_eq!(notice.role, "system");
    assert!(notice.content.contains("Starting logical commits"));
}

#[test]
fn test_commit_push_command_starts_synthetic_user_turn() {
    let mut app = create_test_app();
    app.input = "/commit-push".to_string();
    app.submit_input();

    assert!(app.is_processing);
    assert!(app.pending_turn);
    let notice = app
        .display_messages()
        .last()
        .expect("missing launch notice");
    assert_eq!(notice.role, "system");
    assert!(notice.content.contains("Starting logical commits + push"));
}

#[test]
fn test_help_topic_shows_commit_push_command_details() {
    let mut app = create_test_app();
    app.input = "/help commit-push".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("/commit-push"));
    assert!(msg.content.contains("push"));
}

#[test]
fn test_fast_release_command_starts_synthetic_user_turn() {
    let mut app = create_test_app();
    app.input = "/fast-release".to_string();
    app.submit_input();

    assert!(app.is_processing);
    assert!(app.pending_turn);
    let notice = app
        .display_messages()
        .last()
        .expect("missing launch notice");
    assert_eq!(notice.role, "system");
    assert!(notice
        .content
        .contains("Starting logical commits + push + fast local release"));
}

#[test]
fn test_triage_command_starts_synthetic_user_turn() {
    let mut app = create_test_app();
    app.input = "/triage".to_string();
    app.submit_input();

    assert!(app.is_processing);
    assert!(app.pending_turn);
    let notice = app
        .display_messages()
        .last()
        .expect("missing launch notice");
    assert_eq!(notice.role, "system");
    assert!(notice.content.contains("Starting GitHub issue triage"));
}

#[test]
fn test_triage_command_includes_focus_in_prompt() {
    let prompt = crate::tui::app::commands::build_triage_prompt(" only crash reports");
    assert!(prompt.contains("Triage the open GitHub issues"));
    assert!(prompt.contains("Additional focus from the user: only crash reports"));
}

#[test]
fn test_cut_release_alias_starts_fast_release_turn() {
    let mut app = create_test_app();
    app.input = "/cut-release".to_string();
    app.submit_input();

    assert!(app.is_processing);
    assert!(app.pending_turn);
    let notice = app
        .display_messages()
        .last()
        .expect("missing launch notice");
    assert!(notice.content.contains("fast local release"));
}

#[test]
fn test_fast_release_prompt_uses_selfdev_cache() {
    let fast_prompt = super::commands::build_fast_release_prompt();
    assert!(fast_prompt.contains("quick-release.sh --prepare-fast"));
    assert!(fast_prompt.contains("quick-release.sh --fast-local"));
    assert!(fast_prompt.contains("warm target/selfdev cache"));
    assert!(fast_prompt.contains("Do not run the separate local macOS cross-build"));
    let prepare = fast_prompt.find("--prepare-fast").unwrap();
    let bump = fast_prompt.find("Bump the version").unwrap();
    assert!(prepare < bump);
}

#[test]
fn test_remote_release_command_uses_tag_only_ci_path() {
    let mut app = create_test_app();
    app.input = "/remote-release".to_string();
    app.submit_input();

    assert!(app.is_processing);
    assert!(app.pending_turn);
    let notice = app
        .display_messages()
        .last()
        .expect("missing launch notice");
    assert_eq!(notice.role, "system");
    assert!(notice
        .content
        .contains("Starting logical commits + push + remote release"));

    let prompt = super::commands::build_remote_release_prompt();
    assert!(prompt.contains("quick-release.sh --remote"));
    assert!(prompt.contains("without any local build"));
    assert!(prompt.contains("publication gated"));
}

#[test]
fn test_commit_push_release_alias_starts_synthetic_user_turn() {
    let mut app = create_test_app();
    app.input = "/commit-push-release".to_string();
    app.submit_input();

    assert!(app.is_processing);
    assert!(app.pending_turn);
    let notice = app
        .display_messages()
        .last()
        .expect("missing launch notice");
    assert_eq!(notice.role, "system");
    assert!(notice
        .content
        .contains("Starting logical commits + push + fast local release"));
}

#[test]
fn test_help_topic_shows_cut_release_command_details() {
    let mut app = create_test_app();
    app.input = "/help cut-release".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("/fast-release"));
    assert!(msg.content.contains("--prepare-fast"));
    assert!(msg.content.contains("--fast-local"));
    assert!(msg.content.contains("target/selfdev"));
    assert!(msg.content.contains("compatibility alias"));
}

#[test]
fn test_help_topic_shows_remote_release_command_details() {
    let mut app = create_test_app();
    app.input = "/help remote-release".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("/remote-release"));
    assert!(msg.content.contains("--remote"));
    assert!(msg.content.contains("without running any local build"));
    assert!(msg.content.contains("remains a draft"));
}

#[test]
fn test_help_topic_shows_catchup_command_details() {
    let mut app = create_test_app();
    app.input = "/help catchup".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("/catchup"));
    assert!(msg.content.contains("side panel"));
    assert!(msg.content.contains("/catchup next"));
}

#[test]
fn test_help_topic_shows_back_command_details() {
    let mut app = create_test_app();
    app.input = "/help back".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("/back"));
    assert!(msg.content.contains("Catch Up"));
}

#[test]
fn test_catchup_next_queues_resume_for_attention_session() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.is_remote = true;
        app.remote_session_id = Some(app.session.id.clone());

        let mut target = Session::create(None, Some("catchup target".to_string()));
        target.add_message(
            crate::message::Role::User,
            vec![crate::message::ContentBlock::Text {
                text: "Review the implementation and summarize what changed.".to_string(),
                cache_control: None,
            }],
        );
        target.add_message(
            crate::message::Role::Assistant,
            vec![crate::message::ContentBlock::Text {
                text: "I finished the work and need your decision on the next step.".to_string(),
                cache_control: None,
            }],
        );
        target.mark_closed();
        target.save().expect("save catchup target");

        app.input = "/catchup next".to_string();
        app.submit_input();

        let pending = app
            .pending_catchup_resume
            .clone()
            .expect("missing pending catchup resume");
        assert_eq!(pending.target_session_id, target.id);
        assert_eq!(pending.source_session_id, app.remote_session_id);
        assert_eq!(pending.queue_position, Some((1, 1)));
        assert!(pending.show_brief);

        let msg = app
            .display_messages()
            .last()
            .expect("missing catchup queued message");
        assert_eq!(msg.role, "system");
        assert!(msg.content.contains("Queued Catch Up"));
    });
}

#[test]
fn test_back_command_queues_return_without_showing_brief() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.catchup_return_stack.push("session_prev".to_string());

    app.input = "/back".to_string();
    app.submit_input();

    let pending = app
        .pending_catchup_resume
        .clone()
        .expect("missing pending back resume");
    assert_eq!(pending.target_session_id, "session_prev");
    assert_eq!(pending.source_session_id, None);
    assert_eq!(pending.queue_position, None);
    assert!(!pending.show_brief);
}

#[test]
fn test_maybe_show_catchup_after_history_adds_brief_page_and_marks_seen() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.side_panel = test_side_panel_snapshot("plan", "Plan");

        let source_session_id = app.session.id.clone();
        let mut target = Session::create(None, Some("catchup brief".to_string()));
        target.add_message(
            crate::message::Role::User,
            vec![crate::message::ContentBlock::Text {
                text: "Please review the final diff.".to_string(),
                cache_control: None,
            }],
        );
        target.add_message(
            crate::message::Role::Assistant,
            vec![crate::message::ContentBlock::Text {
                text: "The implementation is complete and needs your approval.".to_string(),
                cache_control: None,
            }],
        );
        target.mark_closed();
        target.save().expect("save catchup brief session");
        let target_id = target.id.clone();

        app.begin_in_flight_catchup_resume(PendingCatchupResume {
            target_session_id: target_id.clone(),
            source_session_id: Some(source_session_id),
            queue_position: Some((1, 1)),
            show_brief: true,
        });
        app.maybe_show_catchup_after_history(&target_id);

        assert!(app.in_flight_catchup_resume.is_none());
        assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("catchup"));
        assert_eq!(app.side_panel.pages.len(), 2);
        assert!(app.side_panel.pages.iter().any(|page| page.id == "plan"));

        let page = app.side_panel.focused_page().expect("missing catchup page");
        assert_eq!(page.id, "catchup");
        assert_eq!(page.file_path, format!("catchup://{}", target_id));
        assert!(page.content.contains("# Catch Up"));
        assert!(page.content.contains("Please review the final diff."));
        assert!(page.content.contains("needs your approval"));

        let persisted = Session::load(&target_id).expect("reload catchup target");
        assert!(!crate::catchup::needs_catchup(
            &target_id,
            persisted.updated_at,
            &persisted.status
        ));
    });
}

#[test]
fn test_help_topic_shows_observe_command_details() {
    let mut app = create_test_app();
    app.input = "/help observe".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("/observe"));
    assert!(msg.content.contains("latest tool call or tool result"));
}

#[test]
fn test_help_topic_shows_splitview_command_details() {
    let mut app = create_test_app();
    app.input = "/help splitview".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("/splitview"));
    assert!(
        msg.content
            .contains("mirrors the current chat in the side panel")
    );
}

#[test]
fn test_help_topic_shows_refactor_command_details() {
    let mut app = create_test_app();
    app.input = "/help refactor".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("/refactor [focus]"));
    assert!(msg.content.contains("independent read-only subagent"));
}

#[test]
fn test_save_command_bookmarks_session_with_memory_enabled() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let mut app = create_test_app();
    app.memory_enabled = true;
    app.messages = vec![
        Message::user("u1"),
        Message::assistant_text("a1"),
        Message::user("u2"),
        Message::assistant_text("a2"),
    ];

    app.input = "/save quick-label".to_string();
    app.submit_input();

    assert!(app.session.saved);
    assert_eq!(app.session.save_label.as_deref(), Some("quick-label"));
    let msg = app
        .display_messages()
        .last()
        .expect("missing save response");
    assert!(msg.content.contains("saved as"));
    assert!(msg.content.contains("quick-label"));

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_goals_command_opens_overview_in_side_panel() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("repo");
    std::fs::create_dir_all(&project).expect("project dir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    crate::goal::create_goal(
        crate::goal::GoalCreateInput {
            title: "Ship mobile MVP".to_string(),
            scope: crate::goal::GoalScope::Project,
            ..crate::goal::GoalCreateInput::default()
        },
        Some(&project),
    )
    .expect("create goal");

    let mut app = create_test_app();
    app.session.working_dir = Some(project.display().to_string());
    app.input = "/goals".to_string();
    app.submit_input();

    assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("goals"));
    let msg = app
        .display_messages()
        .last()
        .expect("missing goals message");
    assert!(msg.content.contains("Opened initiatives overview"));

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_mission_and_goal_commands_are_disabled() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let mut app = create_test_app();
    app.input = "/mission make browser control reliable".to_string();
    app.submit_input();
    assert!(!app.is_processing, "/mission must not start a turn");
    assert!(
        !app.pending_queued_dispatch,
        "/mission must not queue dispatch"
    );
    assert!(
        app.queued_messages.is_empty(),
        "/mission must not queue prompts"
    );
    assert!(
        crate::mission::load(&app.session.id)
            .expect("load mission")
            .is_none(),
        "/mission must not create a mission"
    );

    app.input = "/goal status".to_string();
    app.submit_input();
    assert!(!app.is_processing, "/goal must not start a turn");
    assert!(
        !app.pending_queued_dispatch,
        "/goal must not queue dispatch"
    );
    assert!(
        app.queued_messages.is_empty(),
        "/goal must not queue prompts"
    );
    assert!(
        crate::mission::load(&app.session.id)
            .expect("load mission")
            .is_none(),
        "/goal must not create a mission"
    );

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_goals_legacy_alias_is_not_captured_by_goal_mission_alias() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("repo");
    std::fs::create_dir_all(&project).expect("project dir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let mut app = create_test_app();
    app.session.working_dir = Some(project.display().to_string());
    app.input = "/goals".to_string();
    app.submit_input();

    assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("goals"));
    let mission = crate::mission::load(&app.session.id).expect("load mission");
    assert!(
        mission.is_none(),
        "/goals should not create a mission named `s`"
    );

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_test_command_queues_layered_verification_prompt() {
    let mut app = create_test_app();
    app.input = "/test browser control is reliable".to_string();
    app.submit_input();

    assert!(app.pending_queued_dispatch);
    let queued = app.queued_messages.last().expect("missing /test prompt");
    assert!(queued.contains("browser control is reliable"));
    assert!(queued.contains("Reproduction-first"));
    assert!(queued.contains("End-to-end/user-flow smoke tests"));
    assert!(queued.contains("Property-based tests"));
    assert!(queued.contains("Static analysis"));
    assert!(queued.contains("fault injection/chaos"));
    assert!(queued.contains("Final proof packet"));
}

#[test]
fn test_btw_command_requires_question() {
    let mut app = create_test_app();
    app.input = "/btw".to_string();
    app.submit_input();

    let msg = app.display_messages().last().expect("missing btw error");
    assert_eq!(msg.role, "error");
    assert!(msg.content.contains("Usage: /btw <question>"));
}

#[test]
fn test_btw_command_forks_session_with_question() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let mut app = create_test_app();
    app.input = "/btw what did we decide about config?".to_string();
    app.submit_input();

    // Terminal spawning is disabled under cfg(test), so the fork reports the
    // created session with a manual resume hint.
    let msg = app
        .display_messages()
        .last()
        .expect("missing btw fork message");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("created for the next prompt"));
    let session_id = msg
        .content
        .split("jcode --resume ")
        .nth(1)
        .expect("missing resume hint")
        .trim()
        .to_string();
    let restored =
        App::restore_input_for_reload(&session_id).expect("forked session should stage question");
    assert_eq!(restored.input, "what did we decide about config?");
    assert!(restored.submit_on_restore);
    assert!(restored.pending_images.is_empty());

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_fork_command_with_prompt_forks_session() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let mut app = create_test_app();
    app.input = "/fork try the other approach".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing fork message");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("created for the next prompt"));
    let session_id = msg
        .content
        .split("jcode --resume ")
        .nth(1)
        .expect("missing resume hint")
        .trim()
        .to_string();
    let restored =
        App::restore_input_for_reload(&session_id).expect("forked session should stage prompt");
    assert_eq!(restored.input, "try the other approach");
    assert!(restored.submit_on_restore);

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_fork_command_without_prompt_forks_idle_session() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let mut app = create_test_app();
    app.input = "/fork".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing fork message");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("✂ Fork →"));
    let session_id = msg
        .content
        .split("jcode --resume ")
        .nth(1)
        .expect("missing resume hint")
        .trim()
        .to_string();
    assert!(
        App::restore_input_for_reload(&session_id).is_none(),
        "idle fork should not stage a startup submission"
    );

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_split_command_local_is_alias_for_fork() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let mut app = create_test_app();
    app.input = "/split".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing split message");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("✂ Fork →"));

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_git_command_shows_repo_status_for_working_directory() {
    let repo = create_real_git_repo_fixture();
    std::fs::write(repo.path().join("tracked.txt"), "after\n").expect("update tracked file");

    let mut app = create_test_app();
    app.session.working_dir = Some(repo.path().display().to_string());
    submit_git_command_and_wait_for_response(&mut app);

    let msg = app.display_messages().last().expect("missing git response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("/git"));
    assert!(msg.content.contains("tracked.txt"));
}

#[test]
fn test_git_command_works_in_remote_mode_with_accessible_working_directory() {
    let repo = create_real_git_repo_fixture();
    std::fs::write(repo.path().join("tracked.txt"), "after\n").expect("update tracked file");

    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_session_id = Some("ses_remote_git".to_string());
    app.session.working_dir = Some(repo.path().display().to_string());
    submit_git_command_and_wait_for_response(&mut app);

    let msg = app.display_messages().last().expect("missing git response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("/git"));
    assert!(msg.content.contains("tracked.txt"));
    assert!(
        !msg.content
            .contains("currently only available in a local jcode TUI session")
    );
}

fn submit_git_command_and_wait_for_response(app: &mut App) {
    let expected_session_id = if app.is_remote {
        app.remote_session_id
            .clone()
            .unwrap_or_else(|| app.session.id.clone())
    } else {
        app.session.id.clone()
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut bus_rx = crate::bus::Bus::global().subscribe();
    while bus_rx.try_recv().is_ok() {}

    app.input = "/git".to_string();
    app.submit_input();

    rt.block_on(async {
        loop {
            let event = tokio::time::timeout(std::time::Duration::from_secs(2), bus_rx.recv())
                .await
                .expect("timed out waiting for git status bus event")
                .expect("bus should stay open");
            let saw_completion_for_app = matches!(
                &event,
                crate::bus::BusEvent::GitStatusCompleted(completed)
                    if completed.session_id == expected_session_id
            );
            super::local::handle_bus_event(app, Ok(event));
            if saw_completion_for_app {
                break;
            }
        }
    });
}

#[test]
fn test_observe_command_enables_transient_page_without_persisting() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.input = "/observe on".to_string();
        app.submit_input();

        assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("observe"));
        let page = app.side_panel.focused_page().expect("missing observe page");
        assert_eq!(page.title, "Observe");
        assert_eq!(
            page.source,
            crate::side_panel::SidePanelPageSource::Ephemeral
        );
        assert!(
            page.content
                .contains("Waiting for the next tool call or tool result")
        );

        let persisted = crate::side_panel::snapshot_for_session(&app.session.id)
            .expect("load persisted side panel");
        assert!(persisted.pages.is_empty());
        assert!(persisted.focused_page_id.is_none());
    });
}

#[test]
fn test_splitview_command_enables_transient_page_without_persisting() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.input = "/splitview on".to_string();
        app.submit_input();

        assert_eq!(
            app.side_panel.focused_page_id.as_deref(),
            Some("split_view")
        );
        let page = app
            .side_panel
            .focused_page()
            .expect("missing split view page");
        assert_eq!(page.title, "Split View");
        assert_eq!(
            page.source,
            crate::side_panel::SidePanelPageSource::Ephemeral
        );
        assert!(page.content.contains("Mirror of the current chat"));

        let persisted = crate::side_panel::snapshot_for_session(&app.session.id)
            .expect("load persisted side panel");
        assert!(persisted.pages.is_empty());
        assert!(persisted.focused_page_id.is_none());
    });
}

#[test]
fn test_splitview_command_off_restores_previous_side_panel_page() {
    let mut app = create_test_app();
    app.set_side_panel_snapshot(test_side_panel_snapshot("plan", "Plan"));

    app.input = "/splitview on".to_string();
    app.submit_input();
    assert_eq!(
        app.side_panel.focused_page_id.as_deref(),
        Some("split_view")
    );
    assert!(app.side_panel.pages.iter().any(|page| page.id == "plan"));

    app.input = "/splitview off".to_string();
    app.submit_input();
    assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("plan"));
    assert!(
        !app.side_panel
            .pages
            .iter()
            .any(|page| page.id == "split_view")
    );
}

#[test]
fn test_splitview_mirrors_chat_and_streaming_text() {
    let mut app = create_test_app();
    app.display_messages = vec![
        DisplayMessage::system("System note".to_string()),
        DisplayMessage::user("What did we decide?".to_string()),
        DisplayMessage::assistant("We decided to ship it.".to_string()),
    ];
    app.bump_display_messages_version();
    app.streaming.streaming_text = "Working on the follow-up now...".to_string();
    app.set_split_view_enabled(true, true);

    let page = app
        .side_panel
        .focused_page()
        .expect("missing split view page");
    assert!(page.content.contains("## System"));
    assert!(page.content.contains("## Prompt 1"));
    assert!(page.content.contains("What did we decide?"));
    assert!(page.content.contains("## Response 1"));
    assert!(page.content.contains("We decided to ship it."));
    assert!(page.content.contains("## Live response"));
    assert!(page.content.contains("Working on the follow-up now..."));
}

#[test]
fn test_splitview_does_not_build_cache_while_disabled() {
    let mut app = create_test_app();
    app.display_messages = vec![
        DisplayMessage::user("What did we decide?".to_string()),
        DisplayMessage::assistant("We decided to ship it.".to_string()),
    ];

    app.bump_display_messages_version();

    assert!(!app.split_view_enabled());
    assert!(app.split_view_markdown.is_empty());
}

#[test]
fn test_splitview_disable_clears_cached_markdown() {
    let mut app = create_test_app();
    app.display_messages = vec![
        DisplayMessage::user("What did we decide?".to_string()),
        DisplayMessage::assistant("We decided to ship it.".to_string()),
    ];
    app.bump_display_messages_version();
    app.set_split_view_enabled(true, true);

    assert!(!app.split_view_markdown.is_empty());

    app.set_split_view_enabled(false, false);

    assert!(app.split_view_markdown.is_empty());
}

#[test]
fn test_observe_command_off_restores_previous_side_panel_page() {
    let mut app = create_test_app();
    app.set_side_panel_snapshot(test_side_panel_snapshot("plan", "Plan"));

    app.input = "/observe on".to_string();
    app.submit_input();
    assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("observe"));
    assert!(app.side_panel.pages.iter().any(|page| page.id == "plan"));

    app.input = "/observe off".to_string();
    app.submit_input();
    assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("plan"));
    assert!(!app.side_panel.pages.iter().any(|page| page.id == "observe"));
}

#[test]
fn test_observe_updates_latest_tool_context_only() {
    let mut app = create_test_app();
    app.input = "/observe on".to_string();
    app.submit_input();

    let tool_call = crate::message::ToolCall {
        id: "tool_1".to_string(),
        name: "read".to_string(),
        input: serde_json::json!({"file_path": "src/main.rs", "start_line": 1, "end_line": 10}),
        intent: None, thought_signature: None, };
    app.observe_tool_call(&tool_call);

    let page = app.side_panel.focused_page().expect("missing observe page");
    assert!(
        page.content
            .contains("Latest tool call emitted by the model")
    );
    assert!(page.content.contains("read"));
    assert!(page.content.contains("src/main.rs"));

    app.observe_tool_result(&tool_call, "1 use std::path::Path;", false, Some("read"));

    let page = app.side_panel.focused_page().expect("missing observe page");
    let token_label = crate::util::format_approx_token_count(crate::util::estimate_tokens(
        "1 use std::path::Path;",
    ));
    assert!(page.content.contains("Latest tool result added to context"));
    assert!(page.content.contains("Status: completed"));
    assert!(page.content.contains("Returned to context"));
    assert!(page.content.contains(&token_label));
    assert!(page.content.contains("1 use std::path::Path;"));
    assert!(
        !page
            .content
            .contains("Latest tool call emitted by the model")
    );
}

#[test]
fn test_observe_ignores_noise_tools_and_preserves_latest_useful_context() {
    let mut app = create_test_app();
    app.input = "/observe on".to_string();
    app.submit_input();

    let read_tool = crate::message::ToolCall {
        id: "tool_read".to_string(),
        name: "read".to_string(),
        input: serde_json::json!({"file_path": "src/main.rs"}),
        intent: None, thought_signature: None, };
    app.observe_tool_result(&read_tool, "fn main() {}", false, Some("read"));
    let before = app
        .side_panel
        .focused_page()
        .expect("missing observe page")
        .content
        .clone();

    let noise_tool = crate::message::ToolCall {
        id: "tool_side_panel".to_string(),
        name: "side_panel".to_string(),
        input: serde_json::json!({"action": "write", "page_id": "plan"}),
        intent: None, thought_signature: None, };
    app.observe_tool_call(&noise_tool);
    app.observe_tool_result(&noise_tool, "ok", false, Some("side_panel"));

    let after = app
        .side_panel
        .focused_page()
        .expect("missing observe page")
        .content
        .clone();
    assert_eq!(after, before);
    assert!(after.contains("fn main() {}"));
    assert!(!after.contains("tool_side_panel"));
}

#[test]
fn test_goals_show_command_focuses_goal_page() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("repo");
    std::fs::create_dir_all(&project).expect("project dir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let goal = crate::goal::create_goal(
        crate::goal::GoalCreateInput {
            title: "Ship mobile MVP".to_string(),
            scope: crate::goal::GoalScope::Project,
            ..crate::goal::GoalCreateInput::default()
        },
        Some(&project),
    )
    .expect("create goal");

    let mut app = create_test_app();
    app.session.working_dir = Some(project.display().to_string());
    app.input = format!("/goals show {}", goal.id);
    app.submit_input();

    assert_eq!(
        app.side_panel.focused_page_id.as_deref(),
        Some(format!("goal.{}", goal.id).as_str())
    );

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_compact_mode_command_updates_local_session_mode() {
    let mut app = create_test_app();

    app.input = "/compact mode semantic".to_string();
    app.submit_input();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let mode = rt.block_on(async { app.registry.compaction().read().await.mode() });
    assert_eq!(mode, crate::config::CompactionMode::Semantic);

    let last = app.display_messages().last().expect("missing response");
    assert_eq!(last.role, "system");
    assert_eq!(last.content, "✓ Compaction mode → semantic");
}

#[test]
fn test_compact_mode_status_shows_local_mode() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let compaction = app.registry.compaction();
        let mut manager = compaction.write().await;
        manager.set_mode(crate::config::CompactionMode::Proactive);
    });

    app.input = "/compact mode".to_string();
    app.submit_input();

    let last = app.display_messages().last().expect("missing response");
    assert!(last.content.contains("Compaction mode: proactive"));
}

#[test]
fn test_fast_on_while_processing_mentions_next_request_locally() {
    let mut app = create_fast_test_app();
    app.is_processing = true;
    app.input = "/fast on".to_string();

    app.submit_input();

    let last = app
        .display_messages()
        .last()
        .expect("missing fast mode response");
    assert_eq!(last.role, "system");
    assert_eq!(
        last.content,
        "✓ Fast mode on (Fast)\nApplies to the next request/turn. The current in-flight request keeps its existing tier."
    );
    assert_eq!(
        app.status_notice(),
        Some("Fast: on (next request)".to_string())
    );
}
