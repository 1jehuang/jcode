#[test]
fn test_finish_turn_without_followup_clears_visible_turn_started() {
    let mut app = create_test_app();
    app.is_processing = true;
    app.visible_turn_started = Some(Instant::now() - Duration::from_secs(15));

    super::local::finish_turn(&mut app);

    assert!(app.visible_turn_started.is_none());
}

#[test]
fn test_finish_turn_does_not_duplicate_existing_poke_followup() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "todo-1".to_string(),
                content: "Keep going".to_string(),
                status: "pending".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
            }],
        )
        .expect("save todos");

        app.auto_poke_incomplete_todos = true;
        app.is_processing = true;
        app.queued_messages.push("existing poke".to_string());
        super::local::finish_turn(&mut app);

        assert_eq!(app.queued_messages(), &["existing poke"]);
    });
}

#[test]
fn test_review_prefers_openai_oauth_gpt_5_4_when_available() {
    with_temp_jcode_home(|| {
        let auth_path = crate::storage::jcode_dir()
            .expect("jcode dir")
            .join("openai-auth.json");
        std::fs::write(
            &auth_path,
            serde_json::json!({
                "openai_accounts": [
                    {
                        "label": "openai-1",
                        "access_token": "at_test",
                        "refresh_token": "rt_test",
                        "account_id": "acct_test"
                    }
                ],
                "active_openai_account": "openai-1"
            })
            .to_string(),
        )
        .expect("write auth file");

        assert_eq!(
            super::commands::preferred_one_shot_review_override(),
            Some(("gpt-5.4".to_string(), "openai".to_string()))
        );
    });
}

#[test]
fn test_pending_split_launch_shows_processing_status_in_ui() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.pending_split_started_at = Some(Instant::now());

    assert!(app.is_processing());
    assert!(crate::tui::TuiState::is_processing(&app));
    assert!(matches!(
        crate::tui::TuiState::status(&app),
        ProcessingStatus::Sending
    ));
    assert!(crate::tui::TuiState::elapsed(&app).is_some());
}

#[test]
fn test_expired_pending_split_launch_no_longer_shows_processing_status() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.pending_split_started_at = Some(Instant::now() - Duration::from_millis(400));

    assert!(!app.is_processing());
    assert!(!crate::tui::TuiState::is_processing(&app));
    assert!(matches!(
        crate::tui::TuiState::status(&app),
        ProcessingStatus::Idle
    ));
    assert!(crate::tui::TuiState::elapsed(&app).is_none());
}

#[test]
fn test_pending_remote_dispatch_counts_as_processing_for_tui_state() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.pending_queued_dispatch = true;

    assert!(app.is_processing());
    assert!(crate::tui::TuiState::is_processing(&app));
    assert!(matches!(
        crate::tui::TuiState::status(&app),
        ProcessingStatus::Sending
    ));
}

#[test]
fn test_startup_message_restore_uses_hidden_system_queue() {
    with_temp_jcode_home(|| {
        let session_id = "startup-hidden-queue-test";
        super::App::save_startup_message_for_session(
            session_id,
            "internal startup prompt".to_string(),
        );

        let restored = super::App::restore_input_for_reload(session_id)
            .expect("startup message should restore");
        assert!(restored.queued_messages.is_empty());
        assert_eq!(
            restored.hidden_queued_system_messages,
            vec!["internal startup prompt".to_string()]
        );
    });
}

#[test]
fn test_review_and_judge_startup_prompts_are_analysis_only() {
    let prompts = [
        super::commands::build_autoreview_startup_message("session_parent"),
        super::commands::build_review_startup_message("session_parent"),
        super::commands::build_autojudge_startup_message("session_parent"),
        super::commands::build_judge_startup_message("session_parent"),
    ];

    for prompt in prompts {
        assert!(prompt.contains("analysis-only"));
        assert!(prompt.contains("Do not do the work yourself"));
        assert!(prompt.contains("Do not modify files or repo state"));
        assert!(prompt.contains("send exactly one DM"));
        assert!(prompt.contains("Do not continue implementation"));
    }
}

#[test]
fn test_autojudge_prompt_is_continue_or_stop_manager() {
    let prompt = super::commands::build_autojudge_startup_message("session_parent");

    assert!(prompt.contains("act like a strong completion manager/reviewer"));
    assert!(prompt.contains("tell it exactly what to do next"));
    assert!(prompt.contains("Default to `CONTINUE:` unless you are genuinely convinced"));
    assert!(prompt.contains("Start with either `CONTINUE:` or `STOP:`"));
    assert!(prompt.contains("Address the DM to the parent agent, not to the user"));
}

#[test]
fn test_judge_startup_prompts_describe_visible_mirror_context() {
    let prompts = [
        super::commands::build_autojudge_startup_message("session_parent"),
        super::commands::build_judge_startup_message("session_parent"),
    ];

    for prompt in prompts {
        assert!(prompt.contains("user-visible mirror of the parent conversation"));
        assert!(prompt.contains("shallow summaries of visible tool calls"));
        assert!(prompt.contains("omits deep tool-result details"));
    }
}

#[test]
fn test_prepare_review_spawned_session_uses_visible_transcript_for_judge_sessions() {
    with_temp_jcode_home(|| {
        for title in ["judge", "autojudge"] {
            let parent_id = format!("parent_{title}_visible_context");
            let child_id = format!("child_{title}_visible_context");
            let tool_id = format!("tool_{title}_visible_context");

            let mut parent = crate::session::Session::create_with_id(
                parent_id.clone(),
                None,
                Some("parent".to_string()),
            );
            parent.add_message(
                Role::User,
                vec![ContentBlock::Text {
                    text: "please review what happened".to_string(),
                    cache_control: None,
                }],
            );
            parent.add_message(
                Role::Assistant,
                vec![
                    ContentBlock::Text {
                        text: "I inspected the repo.".to_string(),
                        cache_control: None,
                    },
                    ContentBlock::ToolUse {
                        id: tool_id.clone(),
                        name: "bash".to_string(),
                        input: serde_json::json!({"command": "git diff --stat"}),
                    },
                ],
            );
            parent.add_message(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: tool_id.clone(),
                    content: "SECRET_TOOL_OUTPUT_SHOULD_NOT_APPEAR".to_string(),
                    is_error: None,
                }],
            );
            parent.add_message(
                Role::Assistant,
                vec![
                    ContentBlock::Reasoning {
                        text: "hidden reasoning should never leak".to_string(),
                    },
                    ContentBlock::Text {
                        text: "Final visible answer.".to_string(),
                        cache_control: None,
                    },
                ],
            );
            parent.save().expect("save parent session");

            let mut child = crate::session::Session::create_with_id(
                child_id.clone(),
                Some(parent_id.clone()),
                Some(title.to_string()),
            );
            child.replace_messages(parent.messages.clone());
            child.compaction = Some(crate::session::StoredCompactionState {
                summary_text: "stale compaction".to_string(),
                openai_encrypted_content: None,
                covers_up_to_turn: 1,
                original_turn_count: 1,
                compacted_count: 1,
            });
            child.save().expect("save child session");

            super::commands::prepare_review_spawned_session(
                &child_id,
                super::commands::build_judge_startup_message(&parent_id),
                None,
                None,
                Some(title.to_string()),
                Some(parent_id.clone()),
            );

            let prepared = crate::session::Session::load(&child_id).expect("reload child session");
            let transcript = prepared
                .messages
                .iter()
                .flat_map(|msg| msg.content.iter())
                .filter_map(|block| match block {
                    ContentBlock::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n\n");

            assert!(transcript.contains("please review what happened"));
            assert!(transcript.contains("I inspected the repo."));
            assert!(transcript.contains("Final visible answer."));
            assert!(transcript.contains("Visible tool call"));
            assert!(transcript.contains("git diff --stat"));
            assert!(!transcript.contains("SECRET_TOOL_OUTPUT_SHOULD_NOT_APPEAR"));
            assert!(!transcript.contains("hidden reasoning should never leak"));
            assert_eq!(prepared.parent_id.as_deref(), Some(parent_id.as_str()));
            assert!(prepared.compaction.is_none());
        }
    });
}

#[test]
fn test_queue_autojudge_remote_targets_original_non_judge_session() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.is_remote = true;

        let mut root = crate::session::Session::create(None, Some("task".to_string()));
        root.save().expect("save root session");

        let mut review =
            crate::session::Session::create(Some(root.id.clone()), Some("review".to_string()));
        review.save().expect("save review session");

        let mut judge =
            crate::session::Session::create(Some(review.id.clone()), Some("judge".to_string()));
        judge.save().expect("save judge session");

        app.session = judge.clone();
        app.remote_session_id = Some(judge.id.clone());
        app.autojudge_enabled = true;

        super::commands::queue_autojudge_remote(&mut app);

        assert_eq!(
            app.pending_split_parent_session_id.as_deref(),
            Some(root.id.as_str())
        );
        let startup = app
            .pending_split_startup_message
            .as_deref()
            .expect("autojudge startup message");
        assert!(startup.contains(root.id.as_str()));
        assert!(!startup.contains(review.id.as_str()));
        assert!(!startup.contains(judge.id.as_str()));
    });
}

#[test]
fn test_new_for_remote_restores_spawn_startup_hints_and_dispatch_state() {
    with_temp_jcode_home(|| {
        let session_id = "session_spawn_child";
        let mut session = crate::session::Session::create_with_id(
            session_id.to_string(),
            None,
            Some("spawn child".to_string()),
        );
        session.save().expect("save spawned child session");

        super::App::save_startup_message_for_session(
            session_id,
            super::commands::build_autojudge_startup_message("session_parent_123"),
        );

        let app = App::new_for_remote(Some(session_id.to_string()));

        assert!(app.pending_queued_dispatch);
        assert!(app.is_processing());
        assert!(app.processing_started.is_some());
        assert!(matches!(
            crate::tui::TuiState::status(&app),
            ProcessingStatus::Sending
        ));
        assert_eq!(app.status_notice(), Some("Autojudge starting".to_string()));
        assert_eq!(app.hidden_queued_system_messages.len(), 1);

        let startup_banner = app
            .display_messages()
            .last()
            .expect("spawned session should show startup banner");
        assert_eq!(startup_banner.role, "system");
        assert_eq!(startup_banner.title.as_deref(), Some("Autojudge"));
        assert!(startup_banner.content.contains("analysis-only"));
        assert!(
            startup_banner
                .content
                .contains("send exactly one DM back telling the parent either to `CONTINUE:`")
        );
        assert!(startup_banner.content.contains("user-visible mirror"));
        assert!(startup_banner.content.contains("session_parent_123"));
    });
}

#[test]
fn test_remote_startup_done_event_does_not_cancel_pending_judge_launch() {
    with_temp_jcode_home(|| {
        let session_id = "session_judge_startup_done_guard";
        let mut session = crate::session::Session::create_with_id(
            session_id.to_string(),
            None,
            Some("judge child".to_string()),
        );
        session.save().expect("save judge child session");

        super::App::save_startup_message_for_session(
            session_id,
            super::commands::build_judge_startup_message("session_parent_guard"),
        );

        let mut app = App::new_for_remote(Some(session_id.to_string()));
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        assert!(app.pending_queued_dispatch);
        assert!(app.is_processing());
        assert_eq!(app.current_message_id, None);
        assert_eq!(app.hidden_queued_system_messages.len(), 1);

        app.handle_server_event(crate::protocol::ServerEvent::Done { id: 1 }, &mut remote);

        assert!(app.pending_queued_dispatch);
        assert!(app.is_processing());
        assert!(matches!(
            crate::tui::TuiState::status(&app),
            ProcessingStatus::Sending
        ));
        assert_eq!(app.current_message_id, None);
        assert_eq!(app.hidden_queued_system_messages.len(), 1);
    });
}

#[test]
fn test_remote_startup_judge_hidden_prompt_dispatches_once_history_is_loaded() {
    with_temp_jcode_home(|| {
        let session_id = "session_judge_startup_dispatch";
        let mut session = crate::session::Session::create_with_id(
            session_id.to_string(),
            None,
            Some("judge child".to_string()),
        );
        session.save().expect("save judge child session");

        super::App::save_startup_message_for_session(
            session_id,
            super::commands::build_judge_startup_message("session_parent_dispatch"),
        );

        let mut app = App::new_for_remote(Some(session_id.to_string()));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();

        assert!(app.pending_queued_dispatch);
        assert!(app.is_processing());
        assert_eq!(app.current_message_id, None);

        app.pending_queued_dispatch = false;
        rt.block_on(super::remote::process_remote_followups(
            &mut app,
            &mut remote,
        ));

        assert!(app.hidden_queued_system_messages.is_empty());
        assert!(app.is_processing());
        assert!(matches!(
            crate::tui::TuiState::status(&app),
            ProcessingStatus::Sending
        ));
        assert!(app.current_message_id.is_some());
    });
}

#[test]
fn test_new_for_remote_fresh_spawn_skips_local_transcript_restore() {
    with_temp_jcode_home(|| {
        let session_id = "session_spawn_fresh_skip";
        let mut session = crate::session::Session::create_with_id(
            session_id.to_string(),
            None,
            Some("spawn fresh".to_string()),
        );
        session.model = Some("gpt-5.4".to_string());
        session.append_stored_message(crate::session::StoredMessage {
            id: "msg_spawn_fresh_skip".to_string(),
            role: crate::message::Role::Assistant,
            content: vec![crate::message::ContentBlock::Text {
                text: "persisted transcript should not be restored locally".to_string(),
                cache_control: None,
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        });
        session.save().expect("save spawned child session");

        super::App::save_startup_message_for_session(
            session_id,
            super::commands::build_autojudge_startup_message("session_parent_123"),
        );

        let app = App::new_for_remote_with_options(Some(session_id.to_string()), true);

        assert_eq!(crate::tui::TuiState::provider_model(&app), "gpt-5.4");
        assert!(app.pending_queued_dispatch);
        assert_eq!(app.hidden_queued_system_messages.len(), 1);
        assert_eq!(app.display_messages().len(), 1);
        let startup_banner = app.display_messages().last().expect("startup banner");
        assert_eq!(startup_banner.role, "system");
        assert_eq!(startup_banner.title.as_deref(), Some("Autojudge"));
        assert!(
            !startup_banner
                .content
                .contains("persisted transcript should not be restored locally")
        );
    });
}

#[test]
fn test_new_for_remote_restores_display_history_without_retaining_session_transcript() {
    with_temp_jcode_home(|| {
        let session_id = "session_remote_restore_lightweight";
        let mut session = crate::session::Session::create_with_id(
            session_id.to_string(),
            None,
            Some("remote resume".to_string()),
        );
        session.model = Some("gpt-5.4".to_string());
        session.append_stored_message(crate::session::StoredMessage {
            id: "msg_remote_restore_1".to_string(),
            role: crate::message::Role::Assistant,
            content: vec![crate::message::ContentBlock::Text {
                text: "persisted transcript should render once".to_string(),
                cache_control: None,
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        });
        session.save().expect("save remote restore session");

        let app = App::new_for_remote_with_options(Some(session_id.to_string()), false);

        assert_eq!(crate::tui::TuiState::provider_model(&app), "gpt-5.4");
        assert_eq!(app.display_messages().len(), 1);
        assert_eq!(
            app.display_messages()[0].content,
            "persisted transcript should render once"
        );
        assert!(app.session.messages.is_empty());
        assert!(app.session.compaction.is_none());
    });
}

#[test]
fn test_restore_session_restores_local_judge_processing_state() {
    with_temp_jcode_home(|| {
        let session_id = "session_local_judge_child";
        let mut session = crate::session::Session::create_with_id(
            session_id.to_string(),
            None,
            Some("judge".to_string()),
        );
        session.save().expect("save child session");

        super::App::save_startup_message_for_session(
            session_id,
            super::commands::build_judge_startup_message("session_parent_local"),
        );

        let mut app = create_test_app();
        app.restore_session(session_id);

        assert!(app.is_processing());
        assert!(app.pending_turn);
        assert!(app.processing_started.is_some());
        assert!(matches!(
            crate::tui::TuiState::status(&app),
            ProcessingStatus::Sending
        ));
        assert_eq!(app.status_notice(), Some("Judge starting".to_string()));
        assert_eq!(app.hidden_queued_system_messages.len(), 1);

        let startup_banner = app
            .display_messages()
            .iter()
            .find(|msg| msg.title.as_deref() == Some("Judge"))
            .expect("judge restore should show startup banner");
        assert!(startup_banner.content.contains("session_parent_local"));
        assert!(startup_banner.content.contains("user-visible mirror"));
    });
}

#[test]
fn test_subagent_command_suggestions_include_manual_launch_and_model_policy() {
    let app = create_test_app();

    let subagent = app.get_suggestions_for("/subagent");
    assert!(subagent.iter().any(|(cmd, _)| cmd == "/subagent "));

    let model = app.get_suggestions_for("/subagent-model ");
    assert!(
        model
            .iter()
            .any(|(cmd, _)| cmd == "/subagent-model inherit")
    );

    let review = app.get_suggestions_for("/review");
    assert!(review.iter().any(|(cmd, _)| cmd == "/review"));

    let judge = app.get_suggestions_for("/judge");
    assert!(judge.iter().any(|(cmd, _)| cmd == "/judge"));

    let autojudge = app.get_suggestions_for("/autojudge");
    assert!(autojudge.iter().any(|(cmd, _)| cmd == "/autojudge status"));
}

fn configure_test_remote_models_with_copilot(app: &mut App) {
    app.is_remote = true;
    app.remote_provider_model = Some("claude-sonnet-4".to_string());
    app.remote_available_entries = vec![
        "claude-sonnet-4-6".to_string(),
        "gpt-5.3-codex".to_string(),
        "claude-opus-4.6".to_string(),
        "gemini-3-pro-preview".to_string(),
        "grok-code-fast-1".to_string(),
    ];
}

fn configure_test_remote_models_with_cursor(app: &mut App) {
    app.is_remote = true;
    app.remote_provider_name = Some("cursor".to_string());
    app.remote_provider_model = Some("composer-1.5".to_string());
    app.remote_available_entries = vec![
        "composer-2-fast".to_string(),
        "composer-2".to_string(),
        "composer-1.5".to_string(),
    ];
    app.remote_model_options = app
        .remote_available_entries
        .iter()
        .cloned()
        .map(|model| crate::provider::ModelRoute {
            model,
            provider: "Cursor".to_string(),
            api_method: "cursor".to_string(),
            available: true,
            detail: String::new(),
            cheapness: None,
        })
        .collect();
}

#[test]
fn test_model_picker_includes_copilot_models_in_remote_mode() {
    let mut app = create_test_app();
    configure_test_remote_models_with_copilot(&mut app);

    app.open_model_picker();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");

    let model_names: Vec<&str> = picker.entries.iter().map(|m| m.name.as_str()).collect();

    assert!(
        model_names.contains(&"claude-opus-4.6"),
        "picker should contain copilot model claude-opus-4.6, got: {:?}",
        model_names
    );
    assert!(
        model_names.contains(&"gemini-3-pro-preview"),
        "picker should contain copilot model gemini-3-pro-preview, got: {:?}",
        model_names
    );
    assert!(
        model_names.contains(&"grok-code-fast-1"),
        "picker should contain copilot model grok-code-fast-1, got: {:?}",
        model_names
    );
}

#[test]
fn test_available_models_updated_event_surfaces_authed_provider_in_remote_model_picker() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_remote = true;
    app.handle_server_event(
        crate::protocol::ServerEvent::AvailableModelsUpdated {
            provider_name: Some("Copilot".to_string()),
            provider_model: Some("claude-opus-4.6".to_string()),
            available_models: vec![
                "claude-opus-4.6".to_string(),
                "grok-code-fast-1".to_string(),
            ],
            available_model_routes: vec![
                crate::provider::ModelRoute {
                    model: "claude-opus-4.6".to_string(),
                    provider: "Copilot".to_string(),
                    api_method: "copilot".to_string(),
                    available: true,
                    detail: String::new(),
                    cheapness: None,
                },
                crate::provider::ModelRoute {
                    model: "grok-code-fast-1".to_string(),
                    provider: "Copilot".to_string(),
                    api_method: "copilot".to_string(),
                    available: true,
                    detail: String::new(),
                    cheapness: None,
                },
            ],
        },
        &mut remote,
    );

    app.open_model_picker();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");

    let copilot_entry = picker
        .entries
        .iter()
        .find(|entry| entry.name == "claude-opus-4.6")
        .expect("copilot model should be shown after AvailableModelsUpdated");

    assert!(
        picker
            .entries
            .iter()
            .any(|entry| entry.name == "grok-code-fast-1"),
        "all auth-updated remote models should appear in /model"
    );
    assert!(copilot_entry.options.iter().any(|route| {
        route.provider == "Copilot" && route.api_method == "copilot" && route.available
    }));
}

#[test]
fn test_remote_model_switch_failure_shows_actionable_guidance() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_remote = true;
    app.handle_server_event(
        crate::protocol::ServerEvent::ModelChanged {
            id: 7,
            model: "claude-opus-4.6".to_string(),
            provider_name: Some("Copilot".to_string()),
            error: Some("credentials expired".to_string()),
        },
        &mut remote,
    );

    assert_eq!(app.status_notice(), Some("Model switch failed".to_string()));

    let last = app.display_messages.last().expect("display message");
    assert_eq!(last.role, "error");
    assert!(last.content.contains("credentials expired"));
    assert!(last.content.contains("/model"));
    assert!(last.content.contains("/login"));
    assert!(last.content.contains("reconnect"));
}

#[test]
fn test_model_picker_remote_falls_back_to_current_model_when_catalog_empty() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_provider_name = Some("openrouter".to_string());
    app.remote_provider_model = Some("anthropic/claude-sonnet-4".to_string());
    app.remote_available_entries.clear();
    app.remote_model_options.clear();

    app.open_model_picker();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should open with current-model fallback");

    assert_eq!(picker.entries.len(), 1);
    assert_eq!(picker.entries[0].name, "anthropic/claude-sonnet-4");
    assert_eq!(picker.entries[0].options.len(), 1);
    assert_eq!(picker.entries[0].options[0].provider, "openrouter");
    assert_eq!(picker.entries[0].options[0].api_method, "current");
    assert!(picker.entries[0].options[0].available);
}
