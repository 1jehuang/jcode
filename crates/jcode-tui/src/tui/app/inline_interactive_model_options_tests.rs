use super::*;

#[test]
fn model_options_export_native_routes_and_levels_without_touching_a_draft() {
    let _env = crate::storage::lock_test_env();
    let _home = ConfigHomeGuard(std::env::var_os("JCODE_HOME"));
    let dir = tempfile::tempdir().unwrap();
    crate::env::set_var("JCODE_HOME", dir.path());
    crate::config::Config::invalidate_cache();
    let mut app = crate::tui::app::tests::create_test_app();
    app.is_remote = true;
    app.remote_provider_name = Some("openai".into());
    app.remote_provider_model = Some("gpt-5.5".into());
    app.remote_session_id = Some("session_fox_1_ab".into());
    app.remote_reasoning_effort = Some("high".into());
    app.remote_model_options = vec![
        model_route("gpt-5.5", "OpenAI", "openai-oauth"),
        model_route("gpt-5.5", "OpenAI API", "openai-api-key"),
        model_route("gpt-future-catalog-2099", "OpenAI", "openai-oauth"),
    ];
    app.input = "preserve this unfinished draft".into();
    app.cursor_pos = 7;
    let choices: serde_json::Value =
        serde_json::from_str(&app.handle_debug_command("model-options")).unwrap();
    let choices = choices["choices"].as_array().unwrap();
    assert!(
        choices
            .iter()
            .any(|choice| choice["selection"]["model"] == "gpt-future-catalog-2099")
    );
    let oauth = choices
        .iter()
        .find(|choice| {
            choice["selection"]["model"] == "gpt-5.5" && choice["api_method"] == "openai-oauth"
        })
        .unwrap();
    let api = choices
        .iter()
        .find(|choice| {
            choice["selection"]["model"] == "gpt-5.5" && choice["api_method"] == "openai-api-key"
        })
        .unwrap();
    assert_ne!(
        oauth["id"], api["id"],
        "same model on different providers stays distinct"
    );
    let native = super::super::model_route_effort_levels(
        "gpt-5.5",
        &picker_option_with_method("OpenAI", "openai-oauth"),
    );
    assert_eq!(
        oauth["reasoning_levels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|level| level["value"].as_str().unwrap())
            .collect::<Vec<_>>(),
        native
    );
    assert_eq!(oauth["reasoning_effort"], "high");
    assert_eq!(app.input, "preserve this unfinished draft");
    assert_eq!(app.cursor_pos, 7);
    assert!(app.inline_interactive_state.is_none());
    assert_eq!(app.remote_model_options.len(), 3);
    app.remote_model_options = vec![model_route("gpt-5.5", "current", "current")];
    let choices: serde_json::Value =
        serde_json::from_str(&app.handle_debug_command("model-options")).unwrap();
    assert!(
        choices["choices"].as_array().unwrap().is_empty(),
        "cold placeholder routes are never actionable"
    );
}

#[test]
fn model_options_export_hydrates_the_same_direct_routes_as_models() {
    let _env = crate::storage::lock_test_env();
    let _home = ConfigHomeGuard(std::env::var_os("JCODE_HOME"));
    let dir = tempfile::tempdir().unwrap();
    crate::env::set_var("JCODE_HOME", dir.path());
    crate::config::Config::invalidate_cache();
    let old = std::env::var_os("JCODE_BEDROCK_ENABLE");
    crate::env::set_var("JCODE_BEDROCK_ENABLE", "1");
    crate::auth::AuthStatus::invalidate_cache();
    let mut app = crate::tui::app::tests::create_test_app();
    app.is_remote = true;
    app.remote_startup_phase = None;
    app.remote_provider_name = Some("OpenAI".into());
    app.remote_available_entries = vec!["amazon.nova-pro-v1:0".into()];
    app.remote_model_options = vec![model_route(
        "amazon.nova-pro-v1:0",
        "OpenAI",
        "remote-catalog",
    )];
    app.input = "keep this draft".into();
    let catalog: serde_json::Value =
        serde_json::from_str(&app.handle_debug_command("model-options")).unwrap();
    assert_eq!(app.input, "keep this draft");
    app.open_model_picker();
    match old {
        Some(value) => crate::env::set_var("JCODE_BEDROCK_ENABLE", value),
        None => crate::env::remove_var("JCODE_BEDROCK_ENABLE"),
    }
    crate::auth::AuthStatus::invalidate_cache();
    assert!(
        app.remote_model_options
            .iter()
            .any(|route| route.model == "amazon.nova-pro-v1:0"
                && route.api_method == "bedrock"
                && route.available)
    );
    assert!(catalog["choices"].as_array().unwrap().iter().any(
        |choice| choice["selection"]["model"] == "amazon.nova-pro-v1:0"
            && choice["api_method"] == "bedrock"
            && choice["available"] == true
    ));
}

#[test]
fn external_model_changed_preserves_queued_prompts_until_the_local_ack() {
    let _env = crate::storage::lock_test_env();
    let _home = ConfigHomeGuard(std::env::var_os("JCODE_HOME"));
    let dir = tempfile::tempdir().unwrap();
    crate::env::set_var("JCODE_HOME", dir.path());
    crate::config::Config::invalidate_cache();
    let rt = tokio::runtime::Runtime::new().unwrap();
    for request in [None, Some(22)] {
        let mut app = crate::tui::app::tests::create_test_app();
        app.is_remote = true;
        app.remote_model_switch_in_flight = true;
        app.pending_automatic_failover_request_id = request;
        app.pending_prompt_after_model_switch = Some(crate::tui::app::input::PreparedInput {
            image_markers: Default::default(),
            routing_input: None,
            pasted_contents: Vec::new(),
            raw_input: "queued prompt".into(),
            expanded: "queued prompt".into(),
            images: Vec::new(),
        });
        let mut remote = rt.block_on(async { crate::tui::backend::RemoteConnection::dummy() });
        let route = crate::provider::RouteSelection::from_model_route(&model_route(
            "external-model",
            "OpenAI",
            "openai-api-key",
        ));
        let event = |id, route_selection| crate::protocol::ServerEvent::ModelChanged {
            id,
            route_selection,
            model: "external-model".into(),
            provider_name: Some("OpenAI".into()),
            error: None,
            resolved_credential: Some(jcode_provider_core::ResolvedCredential::ApiKey),
            provider_oauth_email: None,
            provider_oauth_account_label: None,
            reasoning_effort: Some("high".into()),
            context_window: None,
        };
        app.handle_server_event(event(0, Some(route)), &mut remote);
        assert!(app.remote_model_switch_in_flight);
        assert_eq!(app.pending_automatic_failover_request_id, request);
        assert_eq!(
            app.pending_prompt_after_model_switch
                .as_ref()
                .unwrap()
                .raw_input,
            "queued prompt"
        );
        assert_eq!(app.remote_provider_model.as_deref(), Some("external-model"));
        assert_eq!(
            app.current_route_api_method().as_deref(),
            Some("openai-api-key")
        );
        if request.is_none() {
            app.handle_server_event(event(17, None), &mut remote);
            assert!(
                !app.remote_model_switch_in_flight,
                "only the ordinary local acknowledgement releases the queued prompt"
            );
        }
    }
}
