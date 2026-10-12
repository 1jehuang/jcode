// First-frame session facts seeded from the last launch (`remote_header_hint`).

fn provisional_facts_app(model: &str) -> App {
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_provider_name = Some("Claude".to_string());
    app.remote_provider_model = Some(model.to_string());
    app.remote_reasoning_effort = Some("medium".to_string());
    app.remote_resolved_credential = Some(jcode_provider_core::ResolvedCredential::Oauth);
    app.set_context_limit_and_sync_budget(1_000_000);
    app.remote_session_facts_provisional = true;
    app
}

#[test]
fn provisional_session_facts_render_like_the_settled_line() {
    // The seeded facts must produce the same overscroll facts History would:
    // the server-resolved window, the credential chip, and the effort.
    let app = provisional_facts_app("claude-opus-5-5");
    let data = crate::tui::TuiState::info_widget_data(&app);
    assert_eq!(data.context_limit, Some(1_000_000));
    assert_eq!(data.reasoning_effort.as_deref(), Some("medium"));
    assert_eq!(
        data.auth_method,
        crate::tui::info_widget::AuthMethod::AnthropicOAuth
    );
}

#[test]
fn resume_model_changed_over_provisional_facts_is_not_announced() {
    let mut app = provisional_facts_app("claude-opus-5-5");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    let before = app.display_messages.len();

    app.handle_server_event(
        crate::protocol::ServerEvent::ModelChanged {
            context_window: Some(1_000_000),
            id: 0,
            model: "claude-opus-5-5".to_string(),
            provider_name: Some("Claude".to_string()),
            error: None,
            resolved_credential: Some(jcode_provider_core::ResolvedCredential::Oauth),
            reasoning_effort: Some("medium".to_string()),
        },
        &mut remote,
    );

    assert!(!app.remote_session_facts_provisional);
    assert_eq!(app.context_limit, 1_000_000);
    assert_eq!(
        app.display_messages.len(),
        before,
        "confirming the hinted model must not print a 'Switched to model' line"
    );
}

#[test]
fn model_changed_after_facts_are_confirmed_is_announced() {
    let mut app = provisional_facts_app("claude-opus-5-5");
    app.remote_session_facts_provisional = false;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    let before = app.display_messages.len();

    app.handle_server_event(
        crate::protocol::ServerEvent::ModelChanged {
            context_window: Some(200_000),
            id: 0,
            model: "claude-sonnet-4".to_string(),
            provider_name: Some("Claude".to_string()),
            error: None,
            resolved_credential: None,
            reasoning_effort: None,
        },
        &mut remote,
    );

    assert_eq!(app.display_messages.len(), before + 1);
    assert_eq!(app.context_limit, 200_000);
}

#[test]
fn configured_route_seeds_first_frame_facts_without_a_hint() {
    // No remembered hint (first launch, or the hint described another model):
    // the configured route alone fixes the provider, the pinned credential,
    // and the static window of a known model. Before, the raw route id leaked
    // into the status line as `Oauth:claude Opus 5.5` next to a 200k window.
    let _guard = crate::storage::lock_test_env();
    let prev = std::env::var_os("JCODE_MODEL");
    crate::env::set_var("JCODE_MODEL", "claude-oauth:claude-opus-5-5");
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_provider_model = None;
    app.remote_provider_name = None;
    app.apply_configured_route_facts();
    match prev {
        Some(v) => crate::env::set_var("JCODE_MODEL", v),
        None => crate::env::remove_var("JCODE_MODEL"),
    }

    assert_eq!(app.remote_provider_model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(app.remote_provider_name.as_deref(), Some("claude"));
    assert_eq!(app.context_limit, 1_000_000);
    assert_eq!(
        app.remote_resolved_credential,
        Some(jcode_provider_core::ResolvedCredential::Oauth)
    );
    assert!(app.remote_session_facts_provisional);
    let data = crate::tui::TuiState::info_widget_data(&app);
    assert_eq!(
        data.auth_method,
        crate::tui::info_widget::AuthMethod::AnthropicOAuth
    );
}

#[test]
fn new_session_route_facts_ignore_ambient_active_provider_env() {
    // A client launched from inside another jcode (or after an earlier test
    // activated a provider) inherits `JCODE_ACTIVE_PROVIDER`. That must not
    // outrank the configured `JCODE_PROVIDER` for a brand-new session.
    let _guard = crate::storage::lock_test_env();
    let saved: Vec<_> = ["JCODE_MODEL", "JCODE_PROVIDER", "JCODE_ACTIVE_PROVIDER"]
        .into_iter()
        .map(|key| (key, std::env::var_os(key)))
        .collect();
    crate::env::set_var("JCODE_MODEL", "gpt-5.4");
    crate::env::set_var("JCODE_PROVIDER", "openai");
    crate::env::set_var("JCODE_ACTIVE_PROVIDER", "copilot");

    let app = App::new_for_remote(None);
    let provider = crate::tui::TuiState::provider_name(&app);

    for (key, value) in saved {
        match value {
            Some(value) => crate::env::set_var(key, value),
            None => crate::env::remove_var(key),
        }
    }
    assert_eq!(provider, "openai");
}
