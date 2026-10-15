fn openai_oauth_route(model: &str) -> crate::provider::ModelRoute {
    crate::provider::ModelRoute {
        model: model.to_string(),
        provider: "OpenAI".to_string(),
        api_method: "openai-oauth".to_string(),
        available: true,
        detail: String::new(),
        usage: None,
        cheapness: None,
    }
}

fn claude_oauth_route(model: &str) -> crate::provider::ModelRoute {
    crate::provider::ModelRoute {
        model: model.to_string(),
        provider: "Anthropic".to_string(),
        api_method: "claude-oauth".to_string(),
        available: true,
        detail: String::new(),
        usage: None,
        cheapness: None,
    }
}

#[test]
fn test_remote_account_failover_prompt_does_not_hold_or_retry_same_account() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    app.is_remote = true;
    app.auto_poke_incomplete_todos = true;
    app.auto_poke_default_on = true;
    app.queued_messages
        .push(crate::todo::TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE.to_string());
    app.pending_queued_dispatch = true;
    app.remote_provider_name = Some("Anthropic".to_string());
    app.remote_provider_model = Some("claude-sonnet-4".to_string());
    app.remote_model_options = vec![
        claude_oauth_route("claude-sonnet-4"),
        claude_oauth_route("claude-opus-4"),
        openai_oauth_route("gpt-5.5"),
    ];
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "hi".to_string(),
        images: vec![],
        is_system: false,
        system_reminder: None,
        auto_retry: true,
        retry_attempts: 0,
        retry_at: None,
    });
    app.last_submitted_input = Some("hi".to_string());
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    let prompt = jcode_provider_core::ProviderFailoverPrompt {
        from_provider: "claude".into(),
        from_label: "Anthropic".into(),
        to_provider: "openai".into(),
        to_label: "OpenAI".into(),
        target_route: None,
        reason: "429 rate limit exceeded; retry in 60 seconds".into(),
        estimated_input_chars: 400,
        estimated_input_tokens: 100,
    };

    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id: 21,
            message: prompt.to_error_message(),
            retry_after_secs: Some(60),
        },
        &mut remote,
    );

    assert!(app.rate_limit_reset.is_none());
    assert!(app.rate_limit_pending_message.is_none());
    assert!(!app.auto_poke_incomplete_todos);
    assert!(!app.auto_poke_default_on);
    assert!(app.queued_messages.is_empty());
    assert!(!app.pending_queued_dispatch);
    assert!(!app.schedule_turn_end_followups());
    assert_eq!(app.input, "hi");
    assert!(app.pending_provider_failover.is_none());
    let offer = app
        .pending_fallback_offer
        .as_ref()
        .expect("manual fallback offer");
    assert_eq!(offer.selection.provider_label, "OpenAI");
    let resend = offer
        .remote_resend
        .as_ref()
        .expect("preserved failed payload");
    assert_eq!(resend.content, "hi");
    assert_eq!(resend.raw_input.as_deref(), Some("hi"));
    assert!(app.display_messages().iter().any(|message| {
        message.role == "system"
            && message.content.contains("Anthropic unavailable:")
            && message.content.contains("/model")
    }));
    assert!(
        !app.display_messages()
            .iter()
            .any(|message| { message.content.contains("[jcode-provider-failover]") })
    );
    assert!(!app.display_messages().iter().any(|message| {
        message.role == "system"
            && (message.content.contains("queued retry")
                || message.content.contains("Rate limited"))
    }));
}

fn remote_exact_failover_fixture() -> (App, crate::tui::backend::RemoteConnection) {
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_provider_name = Some("Anthropic".into());
    app.remote_provider_model = Some("claude-sonnet-4".into());
    app.session.route_api_method = Some("claude-oauth@claude-otter".into());
    app.remote_resolved_credential = Some(jcode_provider_core::ResolvedCredential::Oauth);
    let mut target = openai_oauth_route("gpt-5.4");
    target.api_method = "openai-oauth@openai-fox".into();
    app.remote_model_options = vec![target];
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "hi".into(),
        images: vec![],
        is_system: false,
        system_reminder: None,
        auto_retry: false,
        retry_attempts: 0,
        retry_at: None,
    });
    app.last_submitted_input = Some("hi".into());
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    let prompt = jcode_provider_core::ProviderFailoverPrompt {
        from_provider: "claude".into(),
        from_label: "Anthropic".into(),
        to_provider: "openai".into(),
        to_label: "OpenAI fox".into(),
        target_route: Some("openai-oauth@openai-fox:gpt-5.4".into()),
        reason: "OAuth usage exhausted".into(),
        estimated_input_chars: 400,
        estimated_input_tokens: 100,
    };
    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id: 21,
            message: prompt.to_error_message(),
            retry_after_secs: None,
        },
        &mut remote,
    );
    (app, remote)
}

#[test]
fn test_remote_failover_countdown_holds_payload_and_stages_exact_route_after_deadline() {
    with_temp_jcode_home(|| {
        write_test_config("[provider]\ncross_provider_failover = \"countdown\"\n");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let (mut app, _remote) = remote_exact_failover_fixture();
        assert!(app.pending_provider_failover.is_some());
        assert_eq!(
            app.pending_provider_failover
                .as_ref()
                .unwrap()
                .remote_resend
                .as_ref()
                .unwrap()
                .raw_input
                .as_deref(),
            Some("hi")
        );
        assert!(app.pending_fallback_offer.is_none());
        assert!(app.pending_fallback_resend.is_none());
        assert!(app.pending_route_selection.is_none());
        app.maybe_progress_provider_failover_countdown();
        assert!(app.pending_route_selection.is_none());
        let manual = crate::provider::RouteSelection::from_model_route(&openai_oauth_route(
            "manual-only-model",
        ));
        app.pending_route_selection = Some(manual.clone());
        app.pending_provider_failover.as_mut().unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        app.maybe_progress_provider_failover_countdown();
        assert!(app.pending_provider_failover.is_none());
        assert_eq!(app.pending_route_selection.as_ref(), Some(&manual));
        assert_eq!(
            app.pending_automatic_failover_route
                .as_ref()
                .unwrap()
                .routed_model_spec(),
            "openai-oauth@openai-fox:gpt-5.4"
        );
        assert_eq!(app.pending_fallback_resend.as_ref().unwrap().content, "hi");
        assert!(!app.pending_turn);
    });
}

#[test]
fn test_remote_generic_auth_error_arms_countdown_for_exact_pool_route() {
    with_temp_jcode_home(|| {
        write_test_config("[provider]\ncross_provider_failover = \"countdown\"\n");
        seed_failover_openai_fox();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let (mut app, _remote) = remote_exact_failover_fixture();
        let payload = app.pending_provider_failover.take().unwrap().remote_resend;
        assert!(app.offer_fallback_after_error_with_payload("401 invalid API key", payload));
        assert!(app.pending_fallback_offer.is_none());
        let pending = app.pending_provider_failover.as_ref().unwrap();
        assert_eq!(
            pending.prompt.target_route.as_deref(),
            Some("openai-oauth@openai-fox:gpt-5.4")
        );
        assert_eq!(pending.remote_resend.as_ref().unwrap().content, "hi");
    });
}

#[test]
fn test_generic_auto_fallback_skips_excluded_preferred_and_checks_source_pool() {
    with_temp_jcode_home(|| {
        write_test_config("[provider]\ncross_provider_failover = \"countdown\"\n");
        seed_failover_openai_fox();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let (mut app, _remote) = remote_exact_failover_fixture();
        let payload = app.pending_provider_failover.take().unwrap().remote_resend;
        app.session.route_api_method = Some("claude-api".into());
        app.remote_resolved_credential = Some(jcode_provider_core::ResolvedCredential::ApiKey);
        let mut preferred = claude_oauth_route("claude-sonnet-4");
        preferred.api_method = "claude-oauth@claude-otter".into();
        app.remote_model_options.insert(0, preferred);
        let mut pool = crate::auth::account_pool::AccountPool {
            members: vec!["anthropic-api".into(), "openai:openai-fox".into()],
            excluded: vec!["claude:claude-otter".into()],
        };
        pool.save().unwrap();
        assert!(
            app.offer_fallback_after_error_with_payload("401 invalid API key", payload.clone())
        );
        assert_eq!(
            app.pending_provider_failover
                .as_ref()
                .unwrap()
                .prompt
                .target_route
                .as_deref(),
            Some("openai-oauth@openai-fox:gpt-5.4")
        );
        app.pending_provider_failover = None;
        pool.members.retain(|key| key != "anthropic-api");
        pool.excluded.push("anthropic-api".into());
        pool.save().unwrap();
        assert!(app.offer_fallback_after_error_with_payload("401 invalid API key", payload));
        assert!(app.pending_provider_failover.is_none());
        assert_eq!(
            app.pending_fallback_offer
                .as_ref()
                .unwrap()
                .selection
                .api_method,
            "claude-oauth@claude-otter"
        );
    });
}

#[test]
fn test_generic_auto_fallback_respects_explicit_pool_account_order() {
    with_temp_jcode_home(|| {
        write_test_config("[provider]\ncross_provider_failover = \"countdown\"\n");
        seed_failover_openai_fox();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let (mut app, _remote) = remote_exact_failover_fixture();
        let payload = app.pending_provider_failover.take().unwrap().remote_resend;
        let mut earlier_catalog_route = openai_oauth_route("gpt-5.4");
        earlier_catalog_route.api_method = "openai-oauth@openai-otter".into();
        app.remote_model_options.insert(0, earlier_catalog_route);
        crate::auth::account_pool::AccountPool {
            members: vec!["openai:openai-fox".into(), "openai:openai-otter".into()],
            excluded: vec![],
        }
        .save()
        .unwrap();
        assert!(app.offer_fallback_after_error_with_payload("401 invalid API key", payload));
        assert_eq!(
            app.pending_provider_failover
                .as_ref()
                .unwrap()
                .prompt
                .target_route
                .as_deref(),
            Some("openai-oauth@openai-fox:gpt-5.4")
        );
    });
}

#[test]
fn test_generic_quota_fallback_skips_exhausted_account_sibling_model() {
    with_temp_jcode_home(|| {
        write_test_config("[provider]\ncross_provider_failover = \"countdown\"\n");
        seed_failover_openai_fox();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let (mut app, _remote) = remote_exact_failover_fixture();
        let payload = app.pending_provider_failover.take().unwrap().remote_resend;
        app.remote_provider_name = Some("OpenAI".into());
        app.remote_provider_model = Some("gpt-5.4".into());
        app.session.route_api_method = Some("openai-oauth@openai-otter".into());
        let mut exhausted_sibling = openai_oauth_route("gpt-5.2");
        exhausted_sibling.api_method = "openai-oauth@openai-otter".into();
        app.remote_model_options.insert(0, exhausted_sibling);
        crate::auth::account_pool::AccountPool {
            members: vec!["openai:openai-otter".into(), "openai:openai-fox".into()],
            excluded: vec![],
        }
        .save()
        .unwrap();
        assert!(app.offer_fallback_after_error_with_payload(
            "429 usage limit reached; quota exhausted",
            payload
        ));
        let target = app
            .pending_provider_failover
            .as_ref()
            .unwrap()
            .prompt
            .target_route
            .as_deref();
        assert_eq!(target, Some("openai-oauth@openai-fox:gpt-5.4"));
        assert!(app.pending_fallback_offer.is_none());
    });
}

#[test]
fn test_rejected_remote_automatic_route_retains_old_identity_and_drops_payload() {
    with_temp_jcode_home(|| {
        write_test_config("[provider]\ncross_provider_failover = \"countdown\"\n");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let (mut app, mut remote) = remote_exact_failover_fixture();
        app.session.route_api_method = Some("claude-oauth@claude-otter".into());
        app.pending_provider_failover.as_mut().unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        app.maybe_progress_provider_failover_countdown();
        assert_eq!(
            app.session.route_api_method.as_deref(),
            Some("claude-oauth@claude-otter")
        );
        app.remote_model_switch_in_flight = true;
        app.pending_automatic_failover_request_id = Some(22);
        app.pending_fallback_resend
            .as_mut()
            .unwrap()
            .images
            .push(("image/png".into(), "image-data".into()));
        app.handle_server_event(
            crate::protocol::ServerEvent::ModelChanged {
                id: 0,
                model: "unrelated-model".into(),
                provider_name: Some("Other".into()),
                error: None,
                provider_oauth_email: None,
                resolved_credential: None,
                reasoning_effort: None,
                context_window: None,
            },
            &mut remote,
        );
        assert!(app.remote_model_switch_in_flight);
        assert!(app.pending_automatic_failover_route.is_some());
        assert!(app.pending_fallback_resend.is_some());
        assert_eq!(app.pending_automatic_failover_request_id, Some(22));
        app.handle_server_event(
            crate::protocol::ServerEvent::ModelChanged {
                id: 22,
                model: "claude-sonnet-4".into(),
                provider_name: None,
                error: Some("Target is no longer in the auto-switch pool".into()),
                provider_oauth_email: None,
                resolved_credential: None,
                reasoning_effort: None,
                context_window: None,
            },
            &mut remote,
        );
        assert_eq!(
            app.session.route_api_method.as_deref(),
            Some("claude-oauth@claude-otter")
        );
        assert_eq!(
            app.remote_provider_model.as_deref(),
            Some("claude-sonnet-4")
        );
        assert!(app.pending_automatic_failover_route.is_none());
        assert!(app.pending_fallback_resend.is_none());
        assert!(!app.remote_model_switch_in_flight);
        assert_eq!(app.input, "hi");
        assert_eq!(
            app.pending_images,
            vec![("image/png".into(), "image-data".into())]
        );
        assert!(app.pending_automatic_failover_request_id.is_none());
    });
}

#[test]
fn test_remote_failover_esc_cancels_without_route_request_or_resend() {
    with_temp_jcode_home(|| {
        write_test_config("[provider]\ncross_provider_failover = \"countdown\"\n");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let (mut app, mut remote) = remote_exact_failover_fixture();
        rt.block_on(remote::handle_remote_key_event(
            &mut app,
            crossterm::event::KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut remote,
        ))
        .unwrap();
        assert!(app.pending_provider_failover.is_none());
        assert!(app.pending_fallback_resend.is_none());
        assert!(app.pending_route_selection.is_none());
        assert_eq!(app.input, "hi");
    });
}

#[test]
fn test_cancel_image_only_failover_restores_attachments() {
    with_temp_jcode_home(|| {
        write_test_config("[provider]\ncross_provider_failover = \"countdown\"\n");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let (mut app, _remote) = remote_exact_failover_fixture();
        app.input.clear();
        let payload = app
            .pending_provider_failover
            .as_mut()
            .unwrap()
            .remote_resend
            .as_mut()
            .unwrap();
        payload.content.clear();
        payload.raw_input = Some(String::new());
        payload.images = vec![("image/png".into(), "image-data".into())];
        app.cancel_pending_provider_failover("Canceled");
        assert_eq!(app.input, "[image 1]");
        input::insert_input_text(&mut app, "restored ");
        assert!(app.input.contains("[image 1]"));
        assert_eq!(
            app.pending_images,
            vec![("image/png".into(), "image-data".into())]
        );
        assert!(app.pending_provider_failover.is_none());
    });
}

#[test]
fn test_pending_failover_honors_changed_manual_configuration() {
    with_temp_jcode_home(|| {
        write_test_config("[provider]\ncross_provider_failover = \"countdown\"\n");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let (mut app, _remote) = remote_exact_failover_fixture();
        write_test_config("[provider]\ncross_provider_failover = \"manual\"\n");
        app.pending_provider_failover.as_mut().unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        app.maybe_progress_provider_failover_countdown();
        assert!(app.pending_provider_failover.is_none());
        assert!(app.pending_route_selection.is_none());
        assert!(app.pending_fallback_resend.is_none());
    });
}

/// The motivating scenario: a remote session's OpenAI OAuth session expires
/// (token refresh fails, non-retryable), and a working Claude OAuth route
/// exists. The terminal error should arm a one-keypress fallback offer that
/// carries the failed payload for resend.
#[test]
fn test_remote_auth_error_arms_fallback_offer_with_resend_payload() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_remote = true;
    app.remote_provider_name = Some("OpenAI".to_string());
    app.remote_provider_model = Some("gpt-5.5".to_string());
    app.remote_model_options = vec![
        openai_oauth_route("gpt-5.5"),
        openai_oauth_route("gpt-5.4"),
        claude_oauth_route("claude-sonnet-4"),
    ];
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "hi".to_string(),
        images: vec![],
        is_system: false,
        system_reminder: None,
        auto_retry: false,
        retry_attempts: 0,
        retry_at: None,
    });
    app.last_submitted_input = Some("hi".to_string());
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;

    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id: 21,
            message: "OpenAI token refresh failed; run /login to re-authenticate: {\"error\":{\"message\":\"Your session has ended. Please log in again.\",\"type\":\"invalid_request_error\",\"code\":\"refresh_token_invalidated\"}}".to_string(),
            retry_after_secs: None,
        },
        &mut remote,
    );

    let offer = app
        .pending_fallback_offer
        .as_ref()
        .expect("terminal auth error should arm a fallback offer");
    // A credential failure must not offer a sibling model behind the same
    // broken OpenAI login; it must hop to the working Anthropic route.
    assert_eq!(offer.selection.provider_label, "Anthropic");
    let resend = offer
        .remote_resend
        .as_ref()
        .expect("remote offer should capture the failed payload");
    assert_eq!(resend.content, "hi");
    assert_eq!(resend.raw_input.as_deref(), Some("hi"));
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.role == "system" && m.content.contains("Fallback available")),
        "offer message should be shown"
    );
}

/// Accepting a remote fallback offer stages the route switch (SetRoute via the
/// dispatcher) and the payload resend; the ModelChanged confirmation then
/// dispatches the resend through process_remote_followups.
#[test]
fn test_remote_fallback_offer_accept_stages_switch_and_resends() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    app.is_remote = true;
    app.remote_provider_name = Some("OpenAI".to_string());
    app.remote_provider_model = Some("gpt-5.5".to_string());
    app.remote_model_options = vec![
        openai_oauth_route("gpt-5.5"),
        claude_oauth_route("claude-sonnet-4"),
    ];
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "hi".to_string(),
        images: vec![],
        is_system: false,
        system_reminder: None,
        auto_retry: false,
        retry_attempts: 0,
        retry_at: None,
    });
    app.last_submitted_input = Some("hi".to_string());
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;

    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id: 22,
            message: "OpenAI token refresh failed; run /login to re-authenticate: refresh_token_invalidated".to_string(),
            retry_after_secs: None,
        },
        &mut remote,
    );
    assert!(app.pending_fallback_offer.is_some());

    assert!(app.apply_pending_fallback_offer());
    assert!(
        app.pending_route_selection.is_some(),
        "accept should stage a SetRoute request for the remote dispatcher"
    );
    let staged = app
        .pending_fallback_resend
        .as_ref()
        .expect("accept should stage the failed payload for resend");
    assert_eq!(staged.content, "hi");

    // Server confirms the switch.
    app.pending_route_selection = None;
    app.remote_model_switch_in_flight = true;
    app.handle_server_event(
        crate::protocol::ServerEvent::ModelChanged {
            context_window: None,
            id: 0,
            model: "claude-sonnet-4".to_string(),
            provider_name: Some("Anthropic".to_string()),
            error: None,
            provider_oauth_email: None,
            resolved_credential: None,
            reasoning_effort: None,
        },
        &mut remote,
    );
    assert!(!app.remote_model_switch_in_flight);

    // The followup dispatcher resends the failed payload on the new route.
    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));
    assert!(app.pending_fallback_resend.is_none());
    assert!(app.is_processing, "resend should start a new turn");
    assert!(matches!(app.status, ProcessingStatus::Sending));
    let pending = app
        .rate_limit_pending_message
        .as_ref()
        .expect("resend should repopulate the pending retry slot");
    assert_eq!(pending.content, "hi");
}

/// A failed route switch must drop the staged resend instead of firing it on
/// the old (broken) route.
#[test]
fn test_remote_fallback_resend_dropped_when_switch_fails() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    app.is_remote = true;
    app.pending_fallback_resend = Some(crate::tui::app::FallbackResendPayload {
        content: "hi".to_string(),
        images: vec![],
        is_system: false,
        auto_retry: false,
        system_reminder: None,
        raw_input: Some("hi".to_string()),
    });
    app.remote_model_switch_in_flight = true;

    app.handle_server_event(
        crate::protocol::ServerEvent::ModelChanged {
            context_window: None,
            id: 0,
            model: "claude-sonnet-4".to_string(),
            provider_name: None,
            error: Some("switch failed".to_string()),
            provider_oauth_email: None,
            resolved_credential: None,
            reasoning_effort: None,
        },
        &mut remote,
    );

    assert!(app.pending_fallback_resend.is_none());
    assert_eq!(
        app.input, "hi",
        "prompt should be restored to the input box"
    );

    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));
    assert!(!app.is_processing, "no resend should fire");
}
