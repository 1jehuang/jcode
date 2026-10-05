#[test]
fn test_agents_review_picker_saves_session_override_without_global_write() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        configure_test_remote_models(&mut app);
        let config_path = crate::storage::jcode_dir().unwrap().join("config.toml");
        let config_before = std::fs::read(&config_path).ok();
        app.open_agent_model_picker(crate::tui::AgentModelTarget::Review);
        app.is_remote = false;

        let selected = app
            .inline_interactive_state
            .as_ref()
            .and_then(|picker| {
                picker.filtered.iter().position(|&idx| {
                    matches!(
                        picker.entries[idx].action,
                        crate::tui::PickerAction::AgentModelChoice {
                            target: crate::tui::AgentModelTarget::Review,
                            clear_override: false,
                        }
                    ) && picker.entries[idx].name != "inherit coordinator"
                })
            })
            .expect("review picker should include at least one model option");
        app.inline_interactive_state.as_mut().unwrap().selected = selected;
        let selected_model_idx = app.inline_interactive_state.as_ref().unwrap().filtered[selected];
        app.inline_interactive_state.as_mut().unwrap().entries[selected_model_idx].options[0]
            .available = true;

        let expected = {
            let picker = app.inline_interactive_state.as_ref().unwrap();
            let entry = &picker.entries[picker.filtered[selected]];
            let base = if entry.effort.is_some() {
                entry
                    .name
                    .rsplit_once(" (")
                    .map(|(base, _)| base.to_string())
                    .unwrap_or_else(|| entry.name.clone())
            } else {
                entry.name.clone()
            };
            let route = &entry.options[entry.selected_option];
            if route.api_method == "copilot" {
                format!("copilot:{}", base)
            } else if route.api_method == "cursor" {
                format!("cursor:{}", base)
            } else if route.api_method == "openai-oauth" {
                format!("openai-oauth:{}", base)
            } else if route.api_method == "openai-api" {
                format!("openai-api:{}", base)
            } else if route.api_method == "claude-oauth" {
                format!("claude-oauth:{}", base)
            } else if route.api_method == "claude-api" && route.provider == "Anthropic" {
                format!("claude-api:{}", base)
            } else if route.api_method == "bedrock" {
                format!("bedrock:{}", base)
            } else if route.api_method == "openrouter" && route.provider != "auto" {
                let catalog_model = crate::provider::openrouter_catalog_model_id(&base)
                    .unwrap_or_else(|| base.clone());
                format!("{}@{}", catalog_model, route.provider)
            } else {
                base
            }
        };

        app.handle_inline_interactive_key(KeyCode::Enter, KeyModifiers::NONE)
            .expect("save agent model override");

        let cfg = crate::config::Config::load();
        assert_eq!(cfg.autoreview.model, None);
        assert_eq!(
            app.session
                .agent_model_overrides
                .get("review")
                .map(String::as_str),
            Some(expected.as_str())
        );
        assert_eq!(std::fs::read(config_path).ok(), config_before);
        assert!(app.inline_interactive_state.is_none());
    });
}

/// Ctrl+G in an `/agents` view switches scope in place, and Enter then
/// writes only that scope: global changes config, session changes the
/// session override.
#[test]
fn test_agents_ctrl_g_toggles_scope_and_saves_to_the_chosen_scope() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        configure_test_remote_models(&mut app);
        app.input = "/agents review".into();
        app.submit_input();
        assert!(!app.agent_models_global_scope);

        // Toggle to the global default and pick "inherit coordinator". The
        // reopen reads the remote catalog configured above.
        app.handle_inline_interactive_key(KeyCode::Char('g'), KeyModifiers::CONTROL)
            .unwrap();
        assert!(app.agent_models_global_scope);
        app.is_remote = false;
        let picker = app.inline_interactive_state.as_mut().expect("still open");
        assert!(
            picker.entries.iter().any(|entry| entry
                .options
                .iter()
                .any(|option| option.detail.starts_with("[global]"))),
            "reopened in global scope"
        );
        picker.selected = picker
            .filtered
            .iter()
            .position(|idx| picker.entries[*idx].name == "inherit coordinator")
            .unwrap();
        app.handle_inline_interactive_key(KeyCode::Enter, KeyModifiers::NONE)
            .unwrap();
        assert_eq!(
            crate::config::Config::load().autoreview.model.as_deref(),
            Some("inherit")
        );
        assert!(app.session.agent_model_overrides.is_empty());

        // The role list toggles too, and the next save stays in session scope.
        app.input = "/agents default".into();
        app.submit_input();
        assert!(app.agent_models_global_scope);
        app.handle_inline_interactive_key(KeyCode::Char('g'), KeyModifiers::CONTROL)
            .unwrap();
        assert!(!app.agent_models_global_scope);
        let roles = app.inline_interactive_state.as_ref().expect("role list");
        assert!(roles.is_agent_target_picker());
        assert!(roles.entries.iter().all(|entry| entry.name.ends_with("[session]")));
        configure_test_remote_models(&mut app);
        app.open_agent_model_picker(crate::tui::AgentModelTarget::Review);
        app.is_remote = false;
        let picker = app.inline_interactive_state.as_mut().unwrap();
        picker.selected = picker
            .filtered
            .iter()
            .position(|idx| picker.entries[*idx].name == "inherit coordinator")
            .unwrap();
        app.handle_inline_interactive_key(KeyCode::Enter, KeyModifiers::NONE)
            .unwrap();
        assert_eq!(
            app.session
                .agent_model_overrides
                .get("review")
                .map(String::as_str),
            Some("inherit")
        );
    });
}

#[test]
fn test_agents_global_scope_and_distinct_session_clear_and_inherit() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        configure_test_remote_models(&mut app);
        // Remote global changes belong to the server: this client must not
        // write its own config (the server write is sent by key handling).
        app.input = "/agents default review".into();
        app.submit_input();
        assert!(app.agent_models_global_scope);
        let picker = app.inline_interactive_state.as_mut().unwrap();
        picker.selected = picker
            .filtered
            .iter()
            .position(|idx| picker.entries[*idx].name == "inherit coordinator")
            .unwrap();
        app.handle_inline_interactive_key(KeyCode::Enter, KeyModifiers::NONE)
            .unwrap();
        assert_eq!(crate::config::Config::load().autoreview.model, None);
        assert!(app.session.agent_model_overrides.is_empty());

        // Locally, the global scope still saves this machine's config.
        app.input = "/agents default review".into();
        app.submit_input();
        app.is_remote = false;
        let picker = app.inline_interactive_state.as_mut().unwrap();
        picker.selected = picker
            .filtered
            .iter()
            .position(|idx| picker.entries[*idx].name == "inherit coordinator")
            .unwrap();
        app.handle_inline_interactive_key(KeyCode::Enter, KeyModifiers::NONE)
            .unwrap();
        assert_eq!(
            crate::config::Config::load().autoreview.model.as_deref(),
            Some("inherit")
        );
        assert!(app.session.agent_model_overrides.is_empty());
        configure_test_remote_models(&mut app);

        app.input = "/agents review".into();
        app.submit_input();
        assert!(!app.agent_models_global_scope);
        app.is_remote = false;
        let picker = app.inline_interactive_state.as_mut().unwrap();
        picker.selected = picker
            .filtered
            .iter()
            .position(|idx| picker.entries[*idx].name == "inherit coordinator")
            .unwrap();
        app.handle_inline_interactive_key(KeyCode::Enter, KeyModifiers::NONE)
            .unwrap();
        assert_eq!(
            app.session
                .agent_model_overrides
                .get("review")
                .map(String::as_str),
            Some("inherit")
        );
        configure_test_remote_models(&mut app);
        app.open_agent_model_picker(crate::tui::AgentModelTarget::Review);
        app.is_remote = false;
        let picker = app.inline_interactive_state.as_mut().unwrap();
        picker.selected = picker
            .filtered
            .iter()
            .position(|idx| {
                matches!(
                    picker.entries[*idx].action,
                    crate::tui::PickerAction::AgentModelChoice {
                        clear_override: true,
                        ..
                    }
                )
            })
            .unwrap();
        app.handle_inline_interactive_key(KeyCode::Enter, KeyModifiers::NONE)
            .unwrap();
        assert!(app.session.agent_model_overrides.is_empty());
        assert_eq!(
            crate::config::Config::load().autoreview.model.as_deref(),
            Some("inherit")
        );
    });
}

#[test]
fn test_agents_remote_feedback_honors_session_global_and_inherit() {
    with_temp_jcode_home(|| {
        let auth_path = crate::storage::jcode_dir()
            .unwrap()
            .join("openai-auth.json");
        std::fs::write(
            auth_path,
            serde_json::json!({
                "openai_accounts": [{
                    "label": "review-test", "access_token": "at_test",
                    "refresh_token": "rt_test", "account_id": "acct_test"
                }],
                "active_openai_account": "review-test"
            })
            .to_string(),
        )
        .unwrap();
        assert!(super::commands::preferred_one_shot_review_override().is_some());
        let mut config = crate::config::Config::load();
        config.autoreview.model = Some("claude-api:global-review".into());
        config.autojudge.model = Some("claude-api:global-judge".into());
        config.save().unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        for (command, role, global) in [
            ("/review", "review", "claude-api:global-review"),
            ("/judge", "judge", "claude-api:global-judge"),
            ("/autoreview now", "review", "claude-api:global-review"),
            ("/autojudge now", "judge", "claude-api:global-judge"),
        ] {
            for (override_model, expected) in [
                (
                    Some("openai-api:session-worker"),
                    Some("openai-api:session-worker"),
                ),
                (Some("inherit"), None),
                (None, Some(global)),
            ] {
                let mut app = create_test_app();
                app.session
                    .set_agent_model_override(role, override_model.map(str::to_string))
                    .unwrap();
                app.is_remote = true;
                app.is_processing = true;
                app.input = command.to_string();
                app.cursor_pos = app.input.len();
                let mut remote = crate::tui::backend::RemoteConnection::dummy();
                rt.block_on(app.handle_remote_key(
                    KeyCode::Enter,
                    KeyModifiers::empty(),
                    &mut remote,
                ))
                .unwrap();
                assert_eq!(
                    app.pending_split_model_override.as_deref(),
                    expected,
                    "{command} with {override_model:?}"
                );
                assert!(
                    app.pending_split_provider_key_override.is_none(),
                    "configured routing must not acquire the legacy preferred reviewer pin"
                );
            }
        }
        config.autoreview.model = Some("inherit".into());
        config.autojudge.model = Some("inherit".into());
        config.save().unwrap();
        for (command, role) in [("/review", "review"), ("/judge", "judge")] {
            let mut app = create_test_app();
            app.session.set_agent_model_override(role, None).unwrap();
            app.is_remote = true;
            app.is_processing = true;
            app.input = command.to_string();
            app.cursor_pos = app.input.len();
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
                .unwrap();
            assert!(
                app.pending_split_model_override.is_none(),
                "{command} must honor global inherit"
            );
            assert!(app.pending_split_provider_key_override.is_none());
        }
    });
}

#[test]
fn test_model_command_suggestions_include_matching_models() {
    let mut app = create_test_app();
    configure_test_remote_models(&mut app);

    let suggestions = app.get_suggestions_for("/model g52c");
    assert_eq!(
        suggestions.first().map(|(cmd, _)| cmd.as_str()),
        Some("/model gpt-5.2-codex")
    );
}

#[test]
fn test_model_command_trailing_space_shows_model_suggestions() {
    let mut app = create_test_app();
    configure_test_remote_models(&mut app);

    let suggestions = app.get_suggestions_for("/model ");
    assert!(
        suggestions
            .iter()
            .any(|(cmd, _)| cmd == "/model gpt-5.3-codex")
    );
}

#[test]
fn test_model_command_provider_suggestions_include_openrouter_routes() {
    let mut app = create_test_app();
    configure_test_remote_openrouter_provider_routes(&mut app);

    let suggestions = app.get_suggestions_for("/model anthropic/claude-sonnet-4@");
    let commands: Vec<&str> = suggestions.iter().map(|(cmd, _)| cmd.as_str()).collect();

    assert!(commands.contains(&"/model anthropic/claude-sonnet-4@auto"));
    assert!(commands.contains(&"/model anthropic/claude-sonnet-4@Fireworks"));
    assert!(commands.contains(&"/model anthropic/claude-sonnet-4@OpenAI"));
}

#[test]
fn test_model_command_provider_suggestions_rank_matching_provider_prefix() {
    let mut app = create_test_app();
    configure_test_remote_openrouter_provider_routes(&mut app);

    let suggestions = app.get_suggestions_for("/model anthropic/claude-sonnet-4@fi");
    assert_eq!(
        suggestions.first().map(|(cmd, _)| cmd.as_str()),
        Some("/model anthropic/claude-sonnet-4@Fireworks")
    );
}

#[test]
fn test_model_command_provider_suggestions_normalize_bare_openai_model_to_openrouter_catalog_id() {
    let (app, _set_model_calls) = create_openrouter_spec_capture_test_app();

    let suggestions = app.get_suggestions_for("/model gpt-5.4@op");
    assert_eq!(
        suggestions.first().map(|(cmd, _)| cmd.as_str()),
        Some("/model openai/gpt-5.4@OpenAI")
    );
}

#[test]
fn test_model_command_provider_suggestions_include_auto_for_normalized_bare_openai_model() {
    let (app, _set_model_calls) = create_openrouter_spec_capture_test_app();

    let suggestions = app.get_suggestions_for("/model gpt-5.4@");
    let commands: Vec<&str> = suggestions.iter().map(|(cmd, _)| cmd.as_str()).collect();

    assert!(commands.contains(&"/model openai/gpt-5.4@auto"));
    assert!(commands.contains(&"/model openai/gpt-5.4@OpenAI"));
}

#[test]
fn test_remote_fallback_provider_suggestions_normalize_bare_openai_openrouter_routes() {
    with_temp_jcode_home(|| {
        let prev_api_key = std::env::var_os("OPENROUTER_API_KEY");
        crate::env::set_var("OPENROUTER_API_KEY", "test-openrouter-key");
        crate::auth::AuthStatus::invalidate_cache();

        let mut app = create_test_app();
        app.is_remote = true;
        app.remote_provider_model = Some("gpt-5.4".to_string());
        app.remote_available_entries = vec!["gpt-5.4".to_string()];
        app.remote_model_options.clear();

        let suggestions = app.get_suggestions_for("/model gpt-5.4@");
        let commands: Vec<&str> = suggestions.iter().map(|(cmd, _)| cmd.as_str()).collect();

        assert!(commands.contains(&"/model openai/gpt-5.4@auto"));
        assert!(commands.contains(&"/model openai/gpt-5.4@OpenAI"));

        if let Some(prev_api_key) = prev_api_key {
            crate::env::set_var("OPENROUTER_API_KEY", prev_api_key);
        } else {
            crate::env::remove_var("OPENROUTER_API_KEY");
        }
        crate::auth::AuthStatus::invalidate_cache();
    });
}

#[test]
fn test_login_command_suggestions_follow_provider_catalog() {
    let app = create_test_app();
    let suggestions = app.get_suggestions_for("/login ");

    for provider in crate::provider_catalog::tui_login_providers() {
        assert!(
            suggestions
                .iter()
                .any(|(cmd, detail)| cmd == &format!("/login {}", provider.id)
                    && detail == &provider.menu_detail),
            "missing /login suggestion for provider {}",
            provider.id
        );
    }
}

#[test]
fn test_model_autocomplete_completes_unique_match() {
    let mut app = create_test_app();
    configure_test_remote_models(&mut app);
    app.input = "/model g52c".to_string();
    app.cursor_pos = app.input.len();

    assert!(app.autocomplete());
    assert_eq!(app.input(), "/model gpt-5.2-codex");
}

#[test]
fn test_model_autocomplete_completes_unique_provider_match() {
    let mut app = create_test_app();
    configure_test_remote_openrouter_provider_routes(&mut app);

    app.input = "/model anthropic/claude-sonnet-4@fi".to_string();
    app.cursor_pos = app.input.len();

    assert!(app.autocomplete());
    assert_eq!(app.input(), "/model anthropic/claude-sonnet-4@Fireworks");
}

#[test]
fn test_model_picker_preview_stays_open_and_updates_filter() {
    let mut app = create_test_app();
    configure_test_remote_models(&mut app);

    for c in "/model g52c".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker preview should be open");
    assert!(picker.preview);
    assert_eq!(picker.filter, "g52c");
    assert!(
        picker
            .filtered
            .iter()
            .any(|&i| picker.entries[i].name.starts_with("gpt-5.2-codex ("))
    );
    assert_eq!(app.input(), "/model g52c");
}

#[test]
fn test_model_picker_preview_enter_selects_model() {
    let mut app = create_test_app();
    configure_test_remote_models(&mut app);

    for c in "/model g52c".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    // Enter from preview mode selects the model and closes the picker
    assert!(app.inline_interactive_state.is_none());
    assert!(app.input().is_empty());
    assert_eq!(app.cursor_pos(), 0);
}
