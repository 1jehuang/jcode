#[test]
fn test_model_picker_preview_arrow_keys_navigate() {
    let mut app = create_test_app();
    configure_test_remote_models(&mut app);

    // Type /model to open preview
    for c in "/model".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker preview should be open");
    assert!(picker.preview);
    let initial_selected = picker.selected;

    // Down arrow should navigate in preview mode
    app.handle_key(KeyCode::Down, KeyModifiers::empty())
        .unwrap();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("picker should still be open");
    assert!(picker.preview, "should remain in preview mode");
    assert_eq!(picker.selected, initial_selected + 1);

    // Up arrow should navigate back
    app.handle_key(KeyCode::Up, KeyModifiers::empty()).unwrap();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("picker should still be open");
    assert!(picker.preview, "should remain in preview mode");
    assert_eq!(picker.selected, initial_selected);

    // Input should be preserved
    assert_eq!(app.input(), "/model");
}

#[test]
fn test_open_model_picker_without_routes_shows_actionable_guidance() {
    let mut app = create_test_app();

    app.open_model_picker();
    wait_for_model_picker_load(&mut app);

    assert!(app.inline_interactive_state.is_none());
    assert_eq!(app.status_notice(), Some("No models available".to_string()));

    let last = app.display_messages.last().expect("display message");
    assert_eq!(last.role, "system");
    assert!(last.content.contains("/login"));
    assert!(last.content.contains("/account"));
    assert!(last.content.contains("/model"));
}

#[derive(Clone)]
struct CountingModelRoutesProvider {
    calls: StdArc<AtomicUsize>,
    route_count: usize,
    delay: Duration,
}

#[async_trait::async_trait]
impl Provider for CountingModelRoutesProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        unimplemented!("CountingModelRoutesProvider")
    }

    fn name(&self) -> &str {
        "counting"
    }

    fn model(&self) -> String {
        "counting-a".to_string()
    }

    fn model_routes(&self) -> Vec<crate::provider::ModelRoute> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
        (0..self.route_count)
            .map(|idx| crate::provider::ModelRoute {
                model: format!("counting-{}", (b'a' + idx as u8) as char),
                provider: "Counting".to_string(),
                api_method: "test".to_string(),
                available: true,
                detail: String::new(),
                cheapness: None,
            })
            .collect()
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

#[test]
fn test_model_picker_reuses_cached_entries_until_invalidated() {
    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let calls = StdArc::new(AtomicUsize::new(0));
    let provider: Arc<dyn Provider> = Arc::new(CountingModelRoutesProvider {
        calls: StdArc::clone(&calls),
        route_count: 2,
        delay: Duration::ZERO,
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;

    app.open_model_picker();
    wait_for_model_picker_load(&mut app);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(app.model_picker_cache.is_some());

    app.open_model_picker();
    wait_for_model_picker_load(&mut app);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "second open should reuse cached picker entries"
    );

    app.invalidate_model_picker_cache();
    app.open_model_picker();
    wait_for_model_picker_load(&mut app);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "invalidating should force rebuilding provider routes"
    );
}

#[test]
fn test_model_picker_opens_loading_state_before_async_routes_complete() {
    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let calls = StdArc::new(AtomicUsize::new(0));
    let provider: Arc<dyn Provider> = Arc::new(CountingModelRoutesProvider {
        calls: StdArc::clone(&calls),
        route_count: 2,
        delay: Duration::from_millis(75),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;

    app.open_model_picker();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("loading picker should open immediately");
    assert_eq!(picker.entries.len(), 1);
    assert_eq!(picker.entries[0].name, "counting-a");
    assert!(picker.entries[0].options[0]
        .detail
        .contains("updating model list"));
    assert!(app.pending_model_picker_load.is_some());
    assert_eq!(app.status_notice(), Some("Updating model list…".to_string()));

    wait_for_model_picker_load(&mut app);
    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("hydrated picker should still be open");
    assert!(picker.entries.len() >= 2);
    assert_eq!(app.status_notice(), Some("Model list updated".to_string()));
}

#[test]
fn test_model_picker_does_not_cache_single_model_fallback() {
    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let calls = StdArc::new(AtomicUsize::new(0));
    let provider: Arc<dyn Provider> = Arc::new(CountingModelRoutesProvider {
        calls: StdArc::clone(&calls),
        route_count: 1,
        delay: Duration::ZERO,
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;

    app.open_model_picker();
    wait_for_model_picker_load(&mut app);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        app.model_picker_cache.is_none(),
        "single-model fallback results should not be retained"
    );

    app.open_model_picker();
    wait_for_model_picker_load(&mut app);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "single-model fallback should be rebuilt so a later full catalog can surface"
    );
}

#[test]
fn test_local_model_picker_selection_failure_keeps_picker_open_and_shows_next_steps() {
    let mut app = create_failing_model_switch_test_app();

    app.open_model_picker();
    wait_for_model_picker_load(&mut app);
    assert!(app.inline_interactive_state.is_some());

    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .expect("enter should be handled");

    assert!(
        app.inline_interactive_state.is_some(),
        "picker should remain open so the user can choose another model"
    );
    assert_eq!(app.status_notice(), Some("Model switch failed".to_string()));

    let last = app.display_messages.last().expect("display message");
    assert_eq!(last.role, "error");
    assert!(last.content.contains("credentials expired"));
    assert!(last.content.contains("/model"));
    assert!(last.content.contains("/login"));
    assert!(last.content.contains("/account"));
}

#[test]
fn test_login_completed_spawns_auth_refresh_when_runtime_is_available() {
    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let started = StdArc::new(AtomicBool::new(false));
    let completed = StdArc::new(AtomicBool::new(false));
    let provider: Arc<dyn Provider> = Arc::new(AsyncAuthRefreshingMockProvider {
        started: StdArc::clone(&started),
        completed: StdArc::clone(&completed),
        delay: Duration::from_millis(150),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;

    let _guard = rt.enter();
    let start = Instant::now();
    app.handle_login_completed(crate::bus::LoginCompleted {
        provider: "openrouter".to_string(),
        success: true,
        message: "OpenRouter ready".to_string(),
    });
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_millis(100),
        "login completion should not block on auth refresh, took {:?}",
        elapsed
    );

    let wait_start = Instant::now();
    while !started.load(Ordering::SeqCst) || !completed.load(Ordering::SeqCst) {
        assert!(
            wait_start.elapsed() < Duration::from_secs(2),
            "background auth refresh did not complete"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn test_login_completed_surfaces_new_provider_models_in_local_model_picker() {
    let mut app = create_auth_refresh_test_app();

    app.handle_login_completed(crate::bus::LoginCompleted {
        provider: "copilot".to_string(),
        success: true,
        message: "Authenticated as **octocat** via GitHub Copilot.\n\nCopilot models are now available in `/model`."
            .to_string(),
    });

    app.open_model_picker();
    wait_for_model_picker_load(&mut app);

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");

    let copilot_entry = picker
        .entries
        .iter()
        .find(|entry| entry.name == "claude-opus-4.6")
        .expect("copilot model should be shown after login");

    assert!(
        picker
            .entries
            .iter()
            .any(|entry| entry.name == "grok-code-fast-1"),
        "all newly available Copilot models should appear in /model"
    );
    assert!(copilot_entry.options.iter().any(|route| {
        route.provider == "Copilot" && route.api_method == "copilot" && route.available
    }));
}

#[test]
fn test_local_model_picker_surfaces_antigravity_models_from_multiprovider() {
    let mut app = create_antigravity_picker_test_app();
    app.open_model_picker();
    wait_for_model_picker_load(&mut app);

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");

    let antigravity_entry = picker
        .entries
        .iter()
        .find(|entry| entry.name == "claude-sonnet-4-6")
        .expect("antigravity model should be shown after login");

    assert!(antigravity_entry.options.iter().any(|route| {
        route.provider == "Antigravity" && route.api_method == "cli" && route.available
    }));
}

#[test]
fn test_local_antigravity_model_picker_selection_preserves_antigravity_provider() {
    let mut app = create_antigravity_picker_test_app();
    app.open_model_picker();
    wait_for_model_picker_load(&mut app);

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");

    let model_idx = picker
        .entries
        .iter()
        .position(|entry| entry.name == "claude-sonnet-4-6")
        .expect("antigravity model should be in picker");
    let filtered_pos = picker
        .filtered
        .iter()
        .position(|&i| i == model_idx)
        .expect("antigravity model should be in filtered list");

    app.inline_interactive_state.as_mut().unwrap().selected = filtered_pos;
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    assert_eq!(app.provider.name(), "Antigravity");
    assert_eq!(app.provider.model(), "claude-sonnet-4-6");
    assert!(app.inline_interactive_state.is_none());
}

#[test]
fn test_local_model_picker_openrouter_bare_openai_route_uses_openai_catalog_prefix() {
    let (mut app, set_model_calls) = create_openrouter_spec_capture_test_app();
    app.open_model_picker();
    wait_for_model_picker_load(&mut app);

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");
    let model_idx = picker
        .entries
        .iter()
        .position(|entry| entry.name == "gpt-5.4 (high)")
        .expect("openrouter-backed OpenAI effort entry should be in picker");
    let filtered_pos = picker
        .filtered
        .iter()
        .position(|&i| i == model_idx)
        .expect("entry should be in filtered list");

    app.inline_interactive_state.as_mut().unwrap().selected = filtered_pos;
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .expect("model picker selection should succeed");

    assert_eq!(
        set_model_calls.lock().unwrap().as_slice(),
        ["openai/gpt-5.4@OpenAI"]
    );
}

#[test]
fn test_agent_model_picker_openrouter_bare_openai_route_saves_openai_catalog_prefix() {
    let (mut app, _set_model_calls) = create_openrouter_spec_capture_test_app();

    app.open_agent_model_picker(crate::tui::AgentModelTarget::Swarm);

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("agent model picker should be open");
    let model_idx = picker
        .entries
        .iter()
        .position(|entry| entry.name == "gpt-5.4 (high)")
        .expect("openrouter-backed OpenAI effort entry should be in picker");
    let filtered_pos = picker
        .filtered
        .iter()
        .position(|&i| i == model_idx)
        .expect("entry should be in filtered list");

    app.inline_interactive_state.as_mut().unwrap().selected = filtered_pos;
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .expect("agent model picker selection should succeed");

    let last = app.display_messages.last().expect("display message");
    assert_eq!(last.role, "system");
    assert!(
        last.content.contains("`openai/gpt-5.4@OpenAI`"),
        "message should show normalized saved spec, got: {}",
        last.content
    );
}

#[test]
fn test_local_model_picker_render_shows_antigravity_models_exactly_as_user_sees_them() {
    let mut app = create_antigravity_picker_test_app();
    let text = render_model_picker_text(&mut app, 90, 12);

    assert!(
        text.contains("ITEM") && text.contains("PROVIDER") && text.contains("ACT"),
        "rendered /model view should include picker columns, got:
{}",
        text
    );
    assert!(
        text.contains("claude-sonnet-4-6"),
        "rendered /model view should show the Antigravity Claude row, got:
{}",
        text
    );
    assert!(
        text.contains("gpt-oss-120b-medium"),
        "rendered /model view should show the Antigravity GPT row, got:
{}",
        text
    );
    assert!(
        text.contains("Antigravity"),
        "rendered /model view should show the Antigravity provider column, got:
{}",
        text
    );
    assert!(
        text.contains("cli"),
        "rendered /model view should show the route transport column, got:
{}",
        text
    );
}

#[test]
fn test_login_picker_preview_stays_open_and_updates_filter() {
    let mut app = create_test_app();

    for c in "/login za".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("login picker preview should be open");
    assert!(picker.preview);
    assert_eq!(picker.kind, crate::tui::PickerKind::Login);
    assert_eq!(picker.filter, "za");
    assert!(
        picker
            .filtered
            .iter()
            .any(|&i| picker.entries[i].name == "Z.AI")
    );
    assert_eq!(app.input(), "/login za");
}

#[test]
fn test_login_picker_preview_enter_starts_login_flow() {
    let mut app = create_test_app();

    for c in "/login zai".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    assert!(app.inline_interactive_state.is_none());
    match app.pending_login {
        Some(crate::tui::app::auth::PendingLogin::ApiKeyProfile {
            provider,
            openai_compatible_profile: Some(profile),
            ..
        }) => {
            assert_eq!(provider, "Z.AI");
            assert_eq!(profile.id, crate::provider_catalog::ZAI_PROFILE.id);
        }
        ref other => panic!("unexpected pending login state: {other:?}"),
    }
}

#[test]
fn test_subagent_model_command_sets_and_resets_session_preference() {
    let mut app = create_test_app();

    assert!(super::commands::handle_session_command(
        &mut app,
        "/subagent-model gpt-5.4"
    ));
    assert_eq!(app.session.subagent_model.as_deref(), Some("gpt-5.4"));

    assert!(super::commands::handle_session_command(
        &mut app,
        "/subagent-model inherit"
    ));
    assert_eq!(app.session.subagent_model, None);
}

#[test]
fn test_autoreview_command_toggles_session_preference() {
    let mut app = create_test_app();

    assert!(super::commands::handle_session_command(
        &mut app,
        "/autoreview on"
    ));
    assert_eq!(app.session.autoreview_enabled, Some(true));
    assert!(app.autoreview_enabled);

    assert!(super::commands::handle_session_command(
        &mut app,
        "/autoreview off"
    ));
    assert_eq!(app.session.autoreview_enabled, Some(false));
    assert!(!app.autoreview_enabled);
}

#[test]
fn test_autojudge_command_toggles_session_preference() {
    let mut app = create_test_app();

    assert!(super::commands::handle_session_command(
        &mut app,
        "/autojudge on"
    ));
    assert_eq!(app.session.autojudge_enabled, Some(true));
    assert!(app.autojudge_enabled);

    assert!(super::commands::handle_session_command(
        &mut app,
        "/autojudge off"
    ));
    assert_eq!(app.session.autojudge_enabled, Some(false));
    assert!(!app.autojudge_enabled);
}

#[test]
fn test_poke_arms_auto_poke_until_todos_are_done() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "todo-1".to_string(),
                content: "Finish the remaining task".to_string(),
                status: "pending".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
            }],
        )
        .expect("save todos");

        assert!(super::commands::handle_session_command(&mut app, "/poke"));

        assert!(app.auto_poke_incomplete_todos);
        assert!(app.pending_turn);
        assert!(app.display_messages().iter().any(|msg| {
            msg.content.contains("Poking model: 1 incomplete todo")
                && msg.content.contains("/poke off")
        }));
    });
}

#[test]
fn test_poke_status_reports_current_state() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "todo-1".to_string(),
                content: "Finish the remaining task".to_string(),
                status: "pending".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
            }],
        )
        .expect("save todos");

        assert!(super::commands::handle_session_command(
            &mut app,
            "/poke status"
        ));
        assert!(app.display_messages().iter().any(|msg| {
            msg.content
                .contains("Auto-poke: **ON**. 1 incomplete todo.")
        }));

        app.auto_poke_incomplete_todos = true;
        app.is_processing = true;
        app.queued_messages
            .push(super::commands::build_poke_message(
                &super::commands::incomplete_poke_todos(&app),
            ));

        assert!(super::commands::handle_session_command(
            &mut app,
            "/poke status"
        ));
        assert!(app.display_messages().iter().any(|msg| {
            msg.content
                .contains("Auto-poke: **ON**. 1 incomplete todo.")
                && msg.content.contains("A follow-up poke is queued.")
                && msg.content.contains("A turn is currently running.")
        }));
    });
}

#[test]
fn test_poke_off_disarms_and_clears_queued_followup() {
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
        app.pending_queued_dispatch = true;
        app.queued_messages
            .push(super::commands::build_poke_message(
                &super::commands::incomplete_poke_todos(&app),
            ));

        assert!(super::commands::handle_session_command(
            &mut app,
            "/poke off"
        ));

        assert!(!app.auto_poke_incomplete_todos);
        assert!(!app.pending_queued_dispatch);
        assert!(app.queued_messages().is_empty());
        assert_eq!(app.status_notice(), Some("Poke: OFF".to_string()));
        assert!(app.display_messages().iter().any(|msg| {
            msg.content.contains("Auto-poke disabled.")
                && msg.content.contains("Cleared 1 queued poke follow-up")
        }));
    });
}

#[test]
fn test_poke_queues_when_turn_is_in_progress() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "todo-1".to_string(),
                content: "Finish the remaining task".to_string(),
                status: "pending".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
            }],
        )
        .expect("save todos");

        app.is_processing = true;

        assert!(super::commands::handle_session_command(&mut app, "/poke"));

        assert!(app.auto_poke_incomplete_todos);
        assert!(app.is_processing);
        assert!(!app.cancel_requested);
        assert!(!app.pending_turn);
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
                    content: "Finish the remaining task".to_string(),
                    status: "pending".to_string(),
                    priority: "high".to_string(),
                    blocked_by: Vec::new(),
                    assigned_to: None,
                },
                crate::todo::TodoItem {
                    id: "todo-2".to_string(),
                    content: "Pick up the newly discovered task".to_string(),
                    status: "pending".to_string(),
                    priority: "medium".to_string(),
                    blocked_by: Vec::new(),
                    assigned_to: None,
                },
            ],
        )
        .expect("save updated todos");

        super::local::finish_turn(&mut app);

        assert!(app.pending_queued_dispatch);
        assert_eq!(app.queued_messages().len(), 1);
        assert!(app.queued_messages()[0].contains("You have 2 incomplete todos"));
        assert!(!app.queued_messages()[0].contains("Pick up the newly discovered task"));
        assert!(!app.queued_messages()[0].contains("/poke off"));
    });
}

#[test]
fn test_finish_turn_auto_pokes_again_when_todos_remain() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "todo-1".to_string(),
                content: "Keep going".to_string(),
                status: "in_progress".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
            }],
        )
        .expect("save todos");

        app.auto_poke_incomplete_todos = true;
        app.is_processing = true;
        super::local::finish_turn(&mut app);

        assert!(app.pending_queued_dispatch);
        assert_eq!(app.queued_messages().len(), 1);
        assert!(app.queued_messages()[0].contains("Continue working, or update the todo tool."));
    });
}

#[test]
fn test_finish_turn_auto_poke_preserves_visible_turn_started() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "todo-1".to_string(),
                content: "Keep going".to_string(),
                status: "in_progress".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
            }],
        )
        .expect("save todos");

        let started = Instant::now() - Duration::from_secs(45);
        app.auto_poke_incomplete_todos = true;
        app.is_processing = true;
        app.visible_turn_started = Some(started);

        super::local::finish_turn(&mut app);

        assert_eq!(app.visible_turn_started, Some(started));
        assert!(app.pending_queued_dispatch);
    });
}
