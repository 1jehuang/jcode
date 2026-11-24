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
