// First-frame session facts seeded from the last launch (`remote_header_hint`).

#[test]
fn ssh_account_header_uses_daemon_identity() {
    with_ssh_remote_test_home(|| {
        let mut app = App::new_for_remote(None);
        app.remote_provider_name = Some("OpenAI".into());
        app.remote_provider_model = Some("gpt-6.1-sol".into());
        app.clear_remote_startup_phase();
        app.remote_provider_oauth_account_label = Some("remote-work".into());
        crate::usage::set_remote_account_usage(
            Vec::new(),
            vec![remote_account_fixture("remote-work")],
        );
        assert_eq!(
            crate::tui::TuiState::startup_oauth_account_label(&app).as_deref(),
            Some("remote-work")
        );
        let (persistent, secondary) = crate::tui::ui::header::build_header_sections(&app, 120);
        let text = persistent
            .iter()
            .chain(secondary.iter())
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(
            text.contains("> "),
            "daemon account must be selected: {text}"
        );
        assert!(text.contains("Account router on test-remote"), "{text}");
        crate::usage::clear_remote_account_usage();
        // Missing daemon identity must not fall back to host-local credentials.
        app.remote_provider_oauth_account_label = None;
        app.session.route_api_method = None;
        assert!(crate::tui::TuiState::startup_oauth_account_label(&app).is_none());
    });
}

#[test]
fn same_model_account_acknowledgement_redraws_and_replaces_identity() {
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut app = provisional_facts_app("gpt-6.1-sol");
        app.remote_session_facts_provisional = false;
        app.remote_provider_oauth_account_label = Some("personal".into());
        app.remote_model_switch_in_flight = true;
        app.remote_available_service_tiers = Some(vec![]);
        app.usage_reset.openai_quota_rejected = true;
        let before = app.display_messages.len();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        for label in [Some("work"), None] {
            let event = serde_json::from_value(serde_json::json!({
                "type": "model_changed",
                "id": 0,
                "model": "gpt-6.1-sol",
                "provider_name": "OpenAI",
                "provider_oauth_account_label": label,
                "resolved_credential": "oauth"
            }))
            .unwrap();
            assert!(app.handle_server_event(event, &mut remote));
            assert!(!app.remote_model_switch_in_flight);
            assert_eq!(app.remote_provider_oauth_account_label.as_deref(), label);
            assert_eq!(
                crate::tui::TuiState::startup_oauth_account_label(&app).as_deref(),
                label
            );
            assert_eq!(
                app.status_notice().as_deref(),
                Some(if label.is_some() {
                    "Account → work"
                } else {
                    "Account updated"
                })
            );
            assert!(app.remote_available_service_tiers.is_none());
            assert!(!app.usage_reset.openai_quota_rejected);
            assert_eq!(app.display_messages.len(), before);
        }
    });
}

#[test]
fn failed_account_acknowledgement_redraws_without_replacing_identity() {
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut app = provisional_facts_app("gpt-6.1-sol");
        app.remote_provider_oauth_account_label = Some("personal".into());
        app.remote_model_switch_in_flight = true;
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        let event = serde_json::from_value(serde_json::json!({
            "type": "model_changed",
            "id": 0,
            "model": "gpt-6.1-sol",
            "error": "Account unavailable",
            "provider_oauth_account_label": "work"
        }))
        .unwrap();
        assert!(app.handle_server_event(event, &mut remote));
        assert!(!app.remote_model_switch_in_flight);
        assert_eq!(
            app.remote_provider_oauth_account_label.as_deref(),
            Some("personal")
        );
    });
}

#[test]
fn remote_oauth_identity_is_hydrated_without_a_saved_route_or_shared_email() {
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        for kind in ["history", "model_changed"] {
            let mut app = create_test_app();
            app.is_remote = true;
            app.session.route_api_method = None;
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            for label in [Some("solosystems"), Some("personal"), None] {
                let event = serde_json::from_value(serde_json::json!({
                    "type": kind,
                    "id": 0,
                    "session_id": "quota-identity-test",
                    "messages": [],
                    "model": "gpt-6.1-sol",
                    "provider_model": "gpt-6.1-sol",
                    "provider_name": "OpenAI",
                    "provider_oauth_account_label": label
                }))
                .unwrap();
                app.handle_server_event(event, &mut remote);
                assert_eq!(app.remote_provider_oauth_account_label.as_deref(), label);
                assert_eq!(
                    crate::tui::TuiState::startup_oauth_account_label(&app).as_deref(),
                    label
                );
                assert!(app.remote_provider_oauth_email.is_none());
                assert!(app.session.route_api_method.is_none());
            }
        }
    });
}

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
            provider_oauth_email: None,
            provider_oauth_account_label: None,
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
            provider_oauth_email: None,
            provider_oauth_account_label: None,
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

    assert_eq!(
        app.remote_provider_model.as_deref(),
        Some("claude-opus-5-5")
    );
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
