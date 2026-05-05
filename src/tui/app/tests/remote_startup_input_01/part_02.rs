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
        cheapness: None,
    }];

    app.handle_server_event(
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
                cheapness: None,
            }],
        },
        &mut remote,
    );

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
        route_count_before: 20,
        route_count_after: 29,
        routes_added: 9,
        routes_removed: 0,
        routes_changed: 2,
    });

    assert!(super::model_context::handle_model_command(
        &mut app,
        "/refresh-model-list"
    ));

    assert_eq!(
        app.status_notice(),
        Some("Model list refreshed: +3 models, +9 routes, ~2 changed".to_string())
    );

    let last = app.display_messages.last().expect("display message");
    assert_eq!(last.role, "system");
    assert!(last.content.contains("**Model List Refresh Complete**"));
    assert!(last.content.contains("Models: 12 → 15  (+3 / -0)"));
    assert!(last.content.contains("Routes: 20 → 29  (+9 / -0 / ~2)"));
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
            cheapness: None,
        }],
    ));

    app.handle_server_event(
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
                    cheapness: None,
                },
                crate::provider::ModelRoute {
                    model: "new-model".to_string(),
                    provider: "OpenRouter".to_string(),
                    api_method: "chat".to_string(),
                    available: true,
                    detail: String::new(),
                    cheapness: None,
                },
            ],
        },
        &mut remote,
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
    assert!(last.content.contains("**Model List Refresh Complete**"));
    assert!(last.content.contains("Models: 1 → 2  (+1 / -0)"));
    assert!(last.content.contains("Routes: 1 → 2  (+1 / -0 / ~1)"));
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
fn test_model_picker_preserves_recommendation_priority_order() {
    let mut app = create_test_app();
    configure_test_remote_models_with_openai_recommendations(&mut app);

    app.open_model_picker();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");

    let model_names: Vec<&str> = picker.entries.iter().map(|m| m.name.as_str()).collect();

    assert_eq!(model_names.first().copied(), Some("gpt-5.2"));

    let gpt55 = picker
        .entries
        .iter()
        .position(|model| model.name == "gpt-5.5")
        .expect("gpt-5.5 should be present");
    let gpt54 = picker
        .entries
        .iter()
        .position(|model| model.name == "gpt-5.4")
        .expect("gpt-5.4 should be present");
    let gpt54_pro = picker
        .entries
        .iter()
        .position(|model| model.name == "gpt-5.4-pro")
        .expect("gpt-5.4-pro should be present");
    let claude_opus = picker
        .entries
        .iter()
        .position(|model| model.name == "claude-opus-4-7")
        .expect("claude-opus-4-7 should be present");
    let spark = picker
        .entries
        .iter()
        .position(|model| model.name == "gpt-5.3-codex-spark")
        .expect("gpt-5.3-codex-spark should be present");
    let codex = picker
        .entries
        .iter()
        .position(|model| model.name == "gpt-5.3-codex")
        .expect("gpt-5.3-codex should be present");

    assert!(
        gpt55 < claude_opus,
        "gpt-5.5 should rank ahead of claude-opus-4-7, got {:?}",
        model_names
    );
    assert!(
        claude_opus < gpt54,
        "claude-opus-4-7 should rank ahead of unrecommended gpt-5.4, got {:?}",
        model_names
    );
    assert!(
        claude_opus < gpt54_pro,
        "claude-opus-4-7 should rank ahead of unrecommended gpt-5.4-pro, got {:?}",
        model_names
    );
    assert!(picker.entries[gpt55].recommended, "gpt-5.5 should be recommended");
    assert!(
        picker.entries[claude_opus].recommended,
        "claude-opus-4-7 should be recommended"
    );
    assert!(!picker.entries[gpt54].recommended, "gpt-5.4 should not be recommended");
    assert!(
        !picker.entries[gpt54_pro].recommended,
        "gpt-5.4-pro should not be recommended"
    );
    assert!(
        !picker.entries[spark].recommended,
        "gpt-5.3-codex-spark should not be recommended"
    );
    assert!(
        !picker.entries[codex].recommended,
        "gpt-5.3-codex should not be recommended"
    );
}
