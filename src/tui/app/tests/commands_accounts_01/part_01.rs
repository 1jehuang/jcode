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
fn session_picker_ctrl_enter_queues_current_terminal_resume_and_closes_overlay() {
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
        crossterm::event::KeyModifiers::CONTROL,
    )
    .expect("session picker ctrl-enter should succeed");

    assert!(app.session_picker_overlay.is_none());
    assert_eq!(
        crate::tui::workspace_client::take_pending_resume_session().as_deref(),
        Some("session_here_123")
    );
}

#[test]
fn test_resize_redraw_is_debounced() {
    let mut app = create_test_app();

    assert!(app.should_redraw_after_resize());
    assert!(!app.should_redraw_after_resize());

    app.last_resize_redraw = Some(Instant::now() - Duration::from_millis(40));
    assert!(app.should_redraw_after_resize());
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
    assert!(msg.content.contains("`/compact`"));
    assert!(msg.content.contains("background"));
    assert!(msg.content.contains("`/compact mode`"));
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
    assert!(msg.content.contains("`/btw <question>`"));
    assert!(msg.content.contains("side panel"));
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
    assert!(msg.content.contains("`/git`"));
    assert!(msg.content.contains("git status --short --branch"));
    assert!(msg.content.contains("`/git status`"));
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
    assert!(msg.content.contains("`/catchup`"));
    assert!(msg.content.contains("side panel"));
    assert!(msg.content.contains("`/catchup next`"));
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
    assert!(msg.content.contains("`/back`"));
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
    assert!(msg.content.contains("`/observe`"));
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
    assert!(msg.content.contains("`/splitview`"));
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
    assert!(msg.content.contains("`/refactor [focus]`"));
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
    assert!(msg.content.contains("Opened goals overview"));

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_btw_command_requires_question() {
    let mut app = create_test_app();
    app.input = "/btw".to_string();
    app.submit_input();

    let msg = app.display_messages().last().expect("missing btw error");
    assert_eq!(msg.role, "error");
    assert!(msg.content.contains("Usage: `/btw <question>`"));
}

#[test]
fn test_btw_command_prepares_side_panel_and_hidden_turn() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let mut app = create_test_app();
    app.input = "/btw what did we decide about config?".to_string();
    app.submit_input();

    assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("btw"));
    let page = app.side_panel.focused_page().expect("missing btw page");
    assert_eq!(page.title, "`/btw`");
    assert!(page.content.contains("## Question"));
    assert!(page.content.contains("what did we decide about config?"));
    assert!(page.content.contains("Thinking…"));
    assert_eq!(app.hidden_queued_system_messages.len(), 1);
    assert!(
        app.hidden_queued_system_messages[0].contains("Question: what did we decide about config?")
    );
    assert!(app.pending_queued_dispatch);

    let msg = app
        .display_messages()
        .last()
        .expect("missing btw status message");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("Running `/btw`"));

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_btw_command_in_remote_mode_queues_followup_instead_of_erroring() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_session_id = Some("ses_remote_btw".to_string());
    app.input = "/btw what are we doing?".to_string();
    app.submit_input();

    assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("btw"));
    assert_eq!(app.hidden_queued_system_messages.len(), 1);
    assert!(app.pending_queued_dispatch);
    let msg = app
        .display_messages()
        .last()
        .expect("missing remote btw message");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("Running `/btw`"));

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
    app.input = "/git".to_string();
    app.submit_input();

    let msg = app.display_messages().last().expect("missing git response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("`/git`"));
    assert!(msg.content.contains("```text"));
    assert!(msg.content.contains("## "));
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
    app.input = "/git".to_string();
    app.submit_input();

    let msg = app.display_messages().last().expect("missing git response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("`/git`"));
    assert!(msg.content.contains("```text"));
    assert!(msg.content.contains("## "));
    assert!(msg.content.contains("tracked.txt"));
    assert!(
        !msg.content
            .contains("currently only available in a local jcode TUI session")
    );
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
    app.streaming_text = "Working on the follow-up now...".to_string();
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
        intent: None,
    };
    app.observe_tool_call(&tool_call);

    let page = app.side_panel.focused_page().expect("missing observe page");
    assert!(
        page.content
            .contains("Latest tool call emitted by the model")
    );
    assert!(page.content.contains("`read`"));
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
        intent: None,
    };
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
        intent: None,
    };
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
    assert!(last.content.contains("Compaction mode: **proactive**"));
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
