#[test]
fn test_handle_server_event_available_models_updated_replaces_remote_model_catalog() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_remote = true;
    app.remote_available_entries = vec!["old-model".to_string()];
    app.remote_model_options = vec![crate::provider::ModelRoute {
        model: "old-model".to_string(),
        provider: "OldProvider".to_string(),
        api_method: "old-api".to_string(),
        available: false,
        detail: "old".to_string(),
        usage: None,
        cheapness: None,
    }];

    let needs_redraw = app.handle_server_event(
        crate::protocol::ServerEvent::AvailableModelsUpdated {
            provider_name: Some("OpenAI".to_string()),
            provider_model: Some("new-model".to_string()),
            available_models: vec!["new-model".to_string(), "second-model".to_string()],
            available_model_routes: vec![crate::provider::ModelRoute {
                model: "new-model".to_string(),
                provider: "OpenAI".to_string(),
                api_method: "openai-oauth".to_string(),
                available: true,
                detail: String::new(),
                usage: None,
                cheapness: None,
            }],
        },
        &mut remote,
    );

    assert!(needs_redraw, "catalog replacement must redraw immediately");
    assert_eq!(
        app.remote_available_entries,
        vec!["new-model".to_string(), "second-model".to_string()]
    );
    assert_eq!(app.remote_model_options.len(), 1);
    assert_eq!(app.remote_model_options[0].model, "new-model");
    assert_eq!(app.remote_model_options[0].provider, "OpenAI");
    assert!(app.remote_model_options[0].available);
    assert_eq!(app.remote_provider_name.as_deref(), Some("OpenAI"));
    assert_eq!(app.remote_provider_model.as_deref(), Some("new-model"));
}

#[test]
fn test_refresh_model_list_command_shows_summary_and_status_notice() {
    let mut app = create_refresh_summary_test_app(crate::provider::ModelCatalogRefreshSummary {
        model_count_before: 12,
        model_count_after: 15,
        models_added: 3,
        models_removed: 0,
        models_added_names: vec![
            "cerebras-fast".to_string(),
            "cerebras-large".to_string(),
            "cerebras-reasoning".to_string(),
        ],
        models_removed_names: Vec::new(),
        route_count_before: 20,
        route_count_after: 29,
        routes_added: 9,
        routes_removed: 0,
        routes_changed: 2,
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut bus_rx = crate::bus::Bus::global().subscribe();
    while bus_rx.try_recv().is_ok() {}

    assert!(super::model_context::handle_model_command(
        &mut app,
        "/refresh-model-list"
    ));

    rt.block_on(async {
        loop {
            let event = tokio::time::timeout(std::time::Duration::from_secs(2), bus_rx.recv())
                .await
                .expect("timed out waiting for model refresh bus event")
                .expect("bus should stay open");
            let saw_completion = matches!(event, crate::bus::BusEvent::ModelRefreshCompleted(_));
            super::local::handle_bus_event(&mut app, Ok(event));
            if saw_completion {
                break;
            }
        }
    });

    assert_eq!(
        app.status_notice(),
        Some("Model list refreshed: +3 models, +9 routes, ~2 changed".to_string())
    );

    let last = app.display_messages.last().expect("display message");
    assert_eq!(last.role, "system");
    assert!(last.content.contains("Model List Refresh Complete"));
    assert!(last.content.contains("Models: 12 → 15  (+3 / -0)"));
    assert!(last.content.contains("Routes: 20 → 29  (+9 / -0 / ~2)"));
    assert!(last.content.contains("Added models:"));
    assert!(last.content.contains("cerebras-fast"));
    assert!(last.content.contains("cerebras-large"));
    assert!(last.content.contains("cerebras-reasoning"));
    assert!(
        !app.display_messages
            .iter()
            .any(|message| message.role == "background_task")
    );
    assert!(app.background_task_rows_ref().iter().any(|row| {
        row.task_id == "refresh-model-list"
            && row.status == crate::tui::BackgroundTaskRowStatus::Completed
    }));
}

#[test]
fn test_remote_available_models_updated_after_refresh_shows_summary_and_updates_catalog() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_remote = true;
    app.pending_remote_model_refresh_snapshot = Some((
        vec!["old-model".to_string()],
        vec![crate::provider::ModelRoute {
            model: "old-model".to_string(),
            provider: "OpenAI".to_string(),
            api_method: "responses".to_string(),
            available: true,
            detail: "old detail".to_string(),
            usage: None,
            cheapness: None,
        }],
    ));

    let needs_redraw = app.handle_server_event(
        crate::protocol::ServerEvent::AvailableModelsUpdated {
            provider_name: None,
            provider_model: None,
            available_models: vec!["old-model".to_string(), "new-model".to_string()],
            available_model_routes: vec![
                crate::provider::ModelRoute {
                    model: "old-model".to_string(),
                    provider: "OpenAI".to_string(),
                    api_method: "responses".to_string(),
                    available: true,
                    detail: "new detail".to_string(),
                    usage: None,
                    cheapness: None,
                },
                crate::provider::ModelRoute {
                    model: "new-model".to_string(),
                    provider: "OpenRouter".to_string(),
                    api_method: "chat".to_string(),
                    available: true,
                    detail: String::new(),
                    usage: None,
                    cheapness: None,
                },
            ],
        },
        &mut remote,
    );

    assert!(
        needs_redraw,
        "model refresh completion must redraw immediately"
    );
    assert_eq!(
        app.status_notice(),
        Some("Model list refreshed: +1 models, +1 routes, ~1 changed".to_string())
    );
    assert_eq!(
        app.remote_available_entries,
        vec!["old-model".to_string(), "new-model".to_string()]
    );
    assert_eq!(app.remote_model_options.len(), 2);
    assert!(app.pending_remote_model_refresh_snapshot.is_none());

    let last = app.display_messages.last().expect("display message");
    assert_eq!(last.role, "system");
    assert!(last.content.contains("Model List Refresh Complete"));
    assert!(last.content.contains("Models: 1 → 2  (+1 / -0)"));
    assert!(last.content.contains("Routes: 1 → 2  (+1 / -0 / ~1)"));
    assert!(last.content.contains("Added models: new-model"));
}

#[test]
fn test_remote_runtime_activity_notification_renders_as_system_message() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::Notification {
            from_session: "jcode".to_string(),
            from_name: Some("Jcode".to_string()),
            notification_type: crate::protocol::NotificationType::Message {
                scope: Some("auth_activity".to_string()),
                channel: None,
                tldr: None,
            },
            message: "**Auth Change Received**\n\nThe server is refreshing provider credentials."
                .to_string(),
        },
        &mut remote,
    );

    let last = app.display_messages.last().expect("display message");
    assert_eq!(last.role, "system");
    assert!(last.content.contains("Auth Change Received"));
    assert_eq!(
        app.status_notice(),
        Some("Auth Change Received".to_string())
    );
}

#[test]
fn test_remote_auth_activity_notification_is_status_only_during_onboarding() {
    let mut app = create_test_app();
    let mut flow = crate::tui::app::onboarding_flow::OnboardingFlow::begin();
    flow.phase = crate::tui::app::onboarding_flow::OnboardingPhase::Login { import: None };
    app.onboarding_flow = Some(flow);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::Notification {
            from_session: "jcode".to_string(),
            from_name: Some("Jcode".to_string()),
            notification_type: crate::protocol::NotificationType::Message {
                scope: Some("auth_activity".to_string()),
                channel: None,
                tldr: None,
            },
            message: "**Auth Change Received**\n\nThe server is refreshing provider credentials."
                .to_string(),
        },
        &mut remote,
    );

    assert!(
        app.display_messages.is_empty(),
        "onboarding should keep auth runtime activity out of chat"
    );
    assert_eq!(
        app.status_notice(),
        Some("Auth Change Received".to_string())
    );
}

#[test]
fn test_remote_final_catalog_activity_is_two_lines_and_completes_model_setup() {
    let mut app = create_test_app();
    app.auth_catalog_refresh_pending = true;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    let message = "**Model ready:** `gpt-5.6-sol`\nOpenAI catalog changed: models +14/-10, routes +24/-19/~3. Use `/model`.";

    app.handle_server_event(
        crate::protocol::ServerEvent::Notification {
            from_session: "jcode".to_string(),
            from_name: Some("Jcode".to_string()),
            notification_type: crate::protocol::NotificationType::Message {
                scope: Some("catalog_activity".to_string()),
                channel: None,
                tldr: None,
            },
            message: message.to_string(),
        },
        &mut remote,
    );

    assert!(!app.auth_catalog_refresh_pending);
    let last = app
        .display_messages
        .last()
        .expect("compact catalog message");
    assert_eq!(last.role, "system");
    assert_eq!(last.content.lines().count(), 2);
    assert_eq!(last.content, message);
}

#[test]
fn test_remote_auth_model_change_does_not_add_a_third_visible_line() {
    let mut app = create_test_app();
    app.auth_catalog_refresh_pending = true;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::ModelChanged {
            context_window: None,
            id: 91,
            model: "gpt-5.6-sol".to_string(),
            provider_name: Some("OpenAI".to_string()),
            error: None,
            resolved_credential: None,
            reasoning_effort: None,
        },
        &mut remote,
    );

    assert_eq!(app.remote_provider_model.as_deref(), Some("gpt-5.6-sol"));
    assert!(app.display_messages.is_empty());
}

#[test]
fn test_remote_onboarding_catalog_activity_completes_model_setup_without_chat_noise() {
    let mut app = create_test_app();
    let mut flow = crate::tui::app::onboarding_flow::OnboardingFlow::begin();
    flow.phase = crate::tui::app::onboarding_flow::OnboardingPhase::Login { import: None };
    app.onboarding_flow = Some(flow);
    app.auth_catalog_refresh_pending = true;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::Notification {
            from_session: "jcode".to_string(),
            from_name: Some("Jcode".to_string()),
            notification_type: crate::protocol::NotificationType::Message {
                scope: Some("catalog_activity".to_string()),
                channel: None,
                tldr: None,
            },
            message: "**Model ready:** `gpt-5.6-sol`\nOpenAI catalog changed: models +14/-10, routes +24/-19/~3. Use `/model`.".to_string(),
        },
        &mut remote,
    );

    assert!(!app.auth_catalog_refresh_pending);
    assert!(app.display_messages.is_empty());
}

#[test]
fn test_remote_catalog_activity_notification_upserts_compact_row() {
    let mut app = create_test_app();
    app.auth_catalog_refresh_pending = true;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    for message in [
        crate::message::format_model_refresh_progress_markdown(
            "Starting provider model catalog refresh",
            Some(5),
        ),
        crate::message::format_model_refresh_progress_markdown(
            "Waiting on provider APIs (2s elapsed)",
            Some(20),
        ),
    ] {
        app.handle_server_event(
            crate::protocol::ServerEvent::Notification {
                from_session: "jcode".to_string(),
                from_name: Some("Jcode".to_string()),
                notification_type: crate::protocol::NotificationType::Message {
                    scope: Some("catalog_activity".to_string()),
                    channel: None,
                    tldr: None,
                },
                message,
            },
            &mut remote,
        );
    }

    assert!(app.display_messages.is_empty());
    assert_eq!(app.background_task_rows_ref().len(), 1);
    assert!(app.auth_catalog_refresh_pending);
    assert_eq!(
        app.background_task_rows_ref()[0],
        crate::tui::BackgroundTaskRow {
            task_id: "refresh-model-list".to_string(),
            label: "Model list refresh".to_string(),
            percent: Some(20.0),
            status: crate::tui::BackgroundTaskRowStatus::Running,
            completed_at: None,
        }
    );
    let status = app.status_notice().expect("status notice");
    assert!(
        status.contains("Waiting on provider APIs (2s elapsed)"),
        "status should summarize latest catalog progress, got: {status}"
    );
}

#[test]
fn test_model_picker_copilot_models_have_copilot_route() {
    let mut app = create_test_app();
    configure_test_remote_models_with_copilot(&mut app);

    app.open_model_picker();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");

    // grok-code-fast-1 is NOT in ALL_CLAUDE_MODELS or ALL_OPENAI_MODELS,
    // so it should get a copilot route
    let grok_entry = picker
        .entries
        .iter()
        .find(|m| m.name == "grok-code-fast-1")
        .expect("grok-code-fast-1 should be in picker");

    assert!(
        grok_entry.options.iter().any(|r| r.api_method == "copilot"),
        "grok-code-fast-1 should have a copilot route, got: {:?}",
        grok_entry.options
    );
}

#[test]
fn test_model_picker_remote_comtegra_model_uses_comtegra_route_not_copilot() {
    let app = with_temp_jcode_home(|| {
        let prev_key = std::env::var("COMTEGRA_API_KEY").ok();
        crate::env::set_var("COMTEGRA_API_KEY", "test-key");

        let mut app = create_test_app();
        app.is_remote = true;
        app.remote_available_entries = vec!["glm-51-nvfp4".to_string()];

        app.open_model_picker();

        match prev_key {
            Some(value) => crate::env::set_var("COMTEGRA_API_KEY", value),
            None => crate::env::remove_var("COMTEGRA_API_KEY"),
        }
        app
    });

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");
    let glm_entry = picker
        .entries
        .iter()
        .find(|m| m.name == "glm-51-nvfp4")
        .expect("glm-51-nvfp4 should be in picker");

    assert!(
        glm_entry.options.iter().any(|r| {
            r.provider == "Comtegra GPU Cloud"
                && r.api_method == "openai-compatible:comtegra"
                && r.available
        }),
        "glm route should be Comtegra/api key, got: {:?}",
        glm_entry.options
    );
    assert!(
        !glm_entry.options.iter().any(|r| r.api_method == "copilot"),
        "glm route should not fall back to Copilot, got: {:?}",
        glm_entry.options
    );
}

#[test]
fn test_model_picker_remote_bedrock_model_has_bedrock_route_when_configured() {
    let _guard = crate::storage::lock_test_env();
    let prev_home = std::env::var("JCODE_HOME").ok();
    let prev_key = std::env::var(crate::provider::bedrock::API_KEY_ENV).ok();
    let prev_region = std::env::var(crate::provider::bedrock::REGION_ENV).ok();
    let temp = tempfile::tempdir().expect("tempdir");
    crate::env::set_var("JCODE_HOME", temp.path().display().to_string());
    crate::env::set_var(crate::provider::bedrock::API_KEY_ENV, "test-bedrock-key");
    crate::env::set_var(crate::provider::bedrock::REGION_ENV, "us-east-2");
    crate::auth::AuthStatus::invalidate_cache();

    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_available_entries = vec!["us.amazon.nova-micro-v1:0".to_string()];

    app.open_model_picker();

    match prev_home {
        Some(value) => crate::env::set_var("JCODE_HOME", value),
        None => crate::env::remove_var("JCODE_HOME"),
    }
    match prev_key {
        Some(value) => crate::env::set_var(crate::provider::bedrock::API_KEY_ENV, value),
        None => crate::env::remove_var(crate::provider::bedrock::API_KEY_ENV),
    }
    match prev_region {
        Some(value) => crate::env::set_var(crate::provider::bedrock::REGION_ENV, value),
        None => crate::env::remove_var(crate::provider::bedrock::REGION_ENV),
    }
    crate::auth::AuthStatus::invalidate_cache();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");
    let nova_entry = picker
        .entries
        .iter()
        .find(|m| m.name == "us.amazon.nova-micro-v1:0")
        .expect("Bedrock Nova model should be in picker");

    assert!(
        nova_entry
            .options
            .iter()
            .any(|r| { r.provider == "AWS Bedrock" && r.api_method == "bedrock" && r.available }),
        "Bedrock route should be available with credentials, got: {:?}",
        nova_entry.options
    );
}

#[test]
fn test_model_picker_preserves_recommendation_priority_order() {
    let _env_guard = crate::storage::lock_test_env();
    let _restore = EnvRestoreGuard::capture(["JCODE_HOME"]);
    let home = tempfile::tempdir().unwrap();
    crate::env::set_var("JCODE_HOME", home.path());
    let mut app = create_test_app();
    configure_test_remote_models_with_openai_recommendations(&mut app);

    app.open_model_picker();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");

    let model_names: Vec<&str> = picker.entries.iter().map(|m| m.name.as_str()).collect();
    let openai_default = jcode_provider_core::DEFAULT_OPENAI_MODEL;
    let claude_default = jcode_provider_core::DEFAULT_CLAUDE_MODEL;
    let position = |prefix: &str, api_method: Option<&str>| {
        picker
            .entries
            .iter()
            .position(|model| {
                (model.name == prefix || model.name.starts_with(&format!("{prefix} (")))
                    && api_method.is_none_or(|method| {
                        model
                            .active_option()
                            .is_some_and(|route| route.api_method == method)
                    })
            })
            .unwrap_or_else(|| panic!("{prefix} {api_method:?} missing from {model_names:?}"))
    };

    let openai_rec = position(openai_default, Some("openai-oauth"));
    let claude_oauth = position(claude_default, Some("claude-oauth"));
    let claude_api = position(claude_default, Some("claude-api"));
    let stale_gpt = position("gpt-5.5", None);
    let stale_opus = position("claude-opus-4-8", None);
    let gpt54 = position("gpt-5.4", None);
    let codex = position("gpt-5.3-codex", None);

    assert!(
        openai_rec < claude_oauth,
        "OpenAI default should rank ahead of Claude default, got {model_names:?}"
    );
    for (index, label) in [
        (stale_gpt, "gpt-5.5"),
        (stale_opus, "claude-opus-4-8"),
        (gpt54, "gpt-5.4"),
        (codex, "gpt-5.3-codex"),
    ] {
        assert!(
            claude_oauth < index && claude_api < index,
            "current defaults should rank ahead of {label}, got {model_names:?}"
        );
        assert!(
            !picker.entries[index].recommended,
            "{label} is no longer a default and should not be recommended"
        );
    }
    for index in [openai_rec, claude_oauth, claude_api] {
        assert!(
            picker.entries[index].recommended,
            "{} should be recommended",
            picker.entries[index].name
        );
    }
    let recommended = picker
        .entries
        .iter()
        .filter(|entry| entry.recommended)
        .count();
    assert_eq!(
        recommended, 3,
        "only the default routes should be recommended, got {model_names:?}"
    );
}
