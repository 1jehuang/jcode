#[test]
fn test_remote_debug_frame_commands_request_a_fresh_draw() {
    for command in [
        "frame",
        "frame-normalized",
        "screen",
        "screen-json",
        "screen-json-normalized",
        "layout",
        "margins",
        "widgets",
        "info-widgets",
        "render-stats",
        "render-order",
        "anomalies",
        "theme",
    ] {
        assert!(
            remote::debug_command_needs_current_frame(command),
            "{command} should capture the current frame"
        );
    }

    for command in ["state", "picker", "memory", "draw-stats", "overlay:on"] {
        assert!(
            !remote::debug_command_needs_current_frame(command),
            "{command} should not force a draw"
        );
    }
}

#[test]
fn test_remote_error_without_retry_recovers_pending_followups() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "retry me".to_string(),
        images: vec![],
        is_system: false,
        system_reminder: None,
        auto_retry: false,
        retry_attempts: 0,
        retry_at: None,
    });
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.current_message_id = Some(10);
    app.interleave_message = Some("unsent interleave".to_string());
    app.pending_soft_interrupts = vec!["acked interleave".to_string()];
    app.pending_soft_interrupt_requests = vec![(88, "acked interleave".to_string())];
    app.queued_messages.push("queued later".to_string());

    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id: 10,
            message: "provider failed hard".to_string(),
            retry_after_secs: None,
        },
        &mut remote,
    );

    assert!(app.rate_limit_pending_message.is_none());
    assert!(app.interleave_message.is_none());
    assert_eq!(
        app.queued_messages(),
        &["unsent interleave", "queued later"]
    );
    assert_eq!(app.pending_soft_interrupts, vec!["acked interleave"]);
    assert_eq!(
        app.pending_soft_interrupt_requests,
        vec![(88, "acked interleave".to_string())]
    );

    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));

    assert!(app.pending_soft_interrupts.is_empty());
    assert!(app.pending_soft_interrupt_requests.is_empty());
    assert!(app.queued_messages().is_empty());
    assert!(app.is_processing);
    assert!(matches!(app.status, ProcessingStatus::Sending));

    let last = app
        .display_messages()
        .last()
        .expect("missing error message");
    assert_eq!(last.role, "user");
    assert_eq!(last.content, "queued later");
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.role == "error" && m.content == "provider failed hard")
    );
}

#[test]
fn test_remote_error_with_retryable_pending_schedules_retry() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "retry me".to_string(),
        images: vec![],
        is_system: true,
        system_reminder: None,
        auto_retry: true,
        retry_attempts: 0,
        retry_at: None,
    });
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;

    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id: 11,
            message: "provider failed hard".to_string(),
            retry_after_secs: None,
        },
        &mut remote,
    );

    let pending = app
        .rate_limit_pending_message
        .as_ref()
        .expect("retryable continuation should remain pending");
    assert!(pending.auto_retry);
    assert_eq!(pending.retry_attempts, 1);
    assert!(pending.retry_at.is_some());
    assert!(app.rate_limit_reset.is_some());
    let retry_notice = app
        .display_messages()
        .iter()
        .find(|message| message.title.as_deref() == Some("Connection"))
        .expect("retry should surface a connection status message");
    assert_eq!(retry_notice.role, "system");
    assert!(retry_notice.content.contains("Connection lost - retrying"));
    assert!(retry_notice.content.contains(&format!(
        "attempt 1/{}",
        App::AUTO_RETRY_MAX_ATTEMPTS
    )));
    assert!(retry_notice.content.contains("Remote request failed"));
}

#[test]
fn test_remote_non_retryable_error_gets_short_auto_poke_retry() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.auto_poke_incomplete_todos = true;
    app.queued_messages
        .push("You have 1 incomplete todo. Continue working, or update the todo tool.".to_string());
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "You have 1 incomplete todo. Continue working, or update the todo tool."
            .to_string(),
        images: vec![],
        is_system: true,
        system_reminder: None,
        auto_retry: true,
        retry_attempts: 0,
        retry_at: None,
    });
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;

    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id: 12,
            message: "OpenAI API error 400 Bad Request: {\"error\":{\"message\":\"Invalid 'input[0].encrypted_content': string too long. Expected a string with maximum length 10485760, but got a string with length 11237432 instead.\",\"type\":\"invalid_request_error\",\"code\":\"string_above_max_length\"}}".to_string(),
            retry_after_secs: None,
        },
        &mut remote,
    );

    assert!(app.auto_poke_incomplete_todos);
    let pending = app
        .rate_limit_pending_message
        .as_ref()
        .expect("deterministic error should get a short retry budget");
    assert_eq!(pending.retry_attempts, 1);
    assert!(app.rate_limit_reset.is_some());
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.role == "system" && m.content.contains("attempt 1/2"))
    );

    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id: 13,
            message: "OpenAI API error 400 Bad Request: {\"error\":{\"type\":\"invalid_request_error\",\"code\":\"string_above_max_length\"}}".to_string(),
            retry_after_secs: None,
        },
        &mut remote,
    );

    assert!(app.auto_poke_incomplete_todos);
    let pending = app
        .rate_limit_pending_message
        .as_ref()
        .expect("second deterministic error should still get final retry");
    assert_eq!(pending.retry_attempts, 2);
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.role == "system" && m.content.contains("attempt 2/2"))
    );
}

#[test]
fn test_remote_non_retryable_error_stops_auto_poke_after_short_retry_budget() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.auto_poke_incomplete_todos = true;
    app.queued_messages
        .push("You have 1 incomplete todo. Continue working, or update the todo tool.".to_string());
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "You have 1 incomplete todo. Continue working, or update the todo tool."
            .to_string(),
        images: vec![],
        is_system: true,
        system_reminder: None,
        auto_retry: true,
        retry_attempts: 2,
        retry_at: None,
    });
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;

    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id: 14,
            message: "OpenAI API error 400 Bad Request: {\"error\":{\"type\":\"invalid_request_error\",\"code\":\"string_above_max_length\"}}".to_string(),
            retry_after_secs: None,
        },
        &mut remote,
    );

    assert!(!app.auto_poke_incomplete_todos);
    assert!(app.queued_messages().is_empty());
    assert!(app.rate_limit_pending_message.is_none());
    assert!(app.rate_limit_reset.is_none());
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.role == "system" && m.content.contains("Auto-poke stopped"))
    );
}

#[test]
fn test_remote_fatal_model_endpoint_error_fails_fast_without_retry_budget() {
    // Volcengine Ark coding-plan endpoint returning 404 UnsupportedModel can
    // never succeed on resend, so the recovery/reconnect continuation must NOT
    // burn the auto-retry budget on it (#387).
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.auto_poke_incomplete_todos = true;
    app.queued_messages
        .push("You have 1 incomplete todo. Continue working, or update the todo tool.".to_string());
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "continue".to_string(),
        images: vec![],
        is_system: true,
        system_reminder: None,
        auto_retry: true,
        retry_attempts: 0,
        retry_at: None,
    });
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;

    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id: 21,
            message: "OpenAI-compatible chat request failed\n  endpoint: https://ark.cn-beijing.volces.com/api/coding/v3/chat/completions\n  model: volcengine:ark-code-latest\n  auth: ARK_API_KEY\n  status: 404 Not Found\n  response: {\"error\":{\"code\":\"UnsupportedModel\",\"message\":\"The requested model does not support the coding plan feature.\"}}".to_string(),
            retry_after_secs: None,
        },
        &mut remote,
    );

    // No retry scheduled: pending cleared immediately, no backoff timer set.
    assert!(app.rate_limit_pending_message.is_none());
    assert!(app.rate_limit_reset.is_none());
    // Auto-poke is stopped, and an actionable hint is shown.
    assert!(!app.auto_poke_incomplete_todos);
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.role == "system" && m.content.contains("Not retrying")),
        "expected a fail-fast model/endpoint hint, got: {:?}",
        app.display_messages()
            .iter()
            .map(|m| m.content.clone())
            .collect::<Vec<_>>()
    );
    // It must not have produced a "retrying (attempt N/M)" message.
    assert!(
        !app.display_messages()
            .iter()
            .any(|m| m.content.contains("attempt 1/"))
    );
}

#[test]
fn test_remote_connectivity_error_waits_for_network_without_retry_budget() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.auto_poke_incomplete_todos = true;
    app.queued_messages
        .push("You have 1 incomplete todo. Continue working, or update the todo tool.".to_string());
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "You have 1 incomplete todo. Continue working, or update the todo tool."
            .to_string(),
        images: vec![],
        is_system: true,
        system_reminder: None,
        auto_retry: true,
        retry_attempts: 0,
        retry_at: None,
    });
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;

    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id: 15,
            message: "Failed to send OpenAI-compatible chat request\n  endpoint: https://api.groq.com/openai/v1/chat/completions\n  model: llama-3.1-8b-instant\n  auth: GROQ_API_KEY\nHint: check network connectivity, DNS/TLS, and that the base URL includes the API version (usually /v1).: error sending request for url (https://api.groq.com/openai/v1/chat/completions): client error (Connect): dns error: failed to lookup address information: Name or service not known".to_string(),
            retry_after_secs: None,
        },
        &mut remote,
    );

    assert!(app.auto_poke_incomplete_todos);
    assert!(!app.queued_messages().is_empty());
    let pending = app
        .rate_limit_pending_message
        .as_ref()
        .expect("offline auto-poke should be held for network recovery");
    assert_eq!(pending.retry_attempts, 0);
    assert!(app.rate_limit_reset.is_some());
    assert!(matches!(
        app.status,
        ProcessingStatus::WaitingForNetwork { .. }
    ));
    assert_eq!(
        app.status_detail.as_deref(),
        Some("offline; waiting for network before retry")
    );
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.role == "system" && m.content.contains("Network appears offline"))
    );
    assert!(
        !app.display_messages()
            .iter()
            .any(|m| m.role == "system" && m.content.contains("attempt 1/2"))
    );
}

#[test]
fn test_remote_connectivity_error_without_auto_retry_still_waits_for_network() {
    // Regression: an auto-poke continuation that carries a visible message gets
    // auto_retry=false. A transient DNS failure must still hold the turn for
    // network recovery instead of permanently stopping auto-poke.
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.auto_poke_incomplete_todos = true;
    app.queued_messages
        .push("You have 1 incomplete todo. Continue working, or update the todo tool.".to_string());
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "Continue working on the task.".to_string(),
        images: vec![],
        is_system: true,
        system_reminder: None,
        auto_retry: false,
        retry_attempts: 0,
        retry_at: None,
    });
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;

    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id: 16,
            message: "Failed to send request to Anthropic API: error sending request for url (https://api.anthropic.com/v1/messages): client error (Connect): dns error: failed to lookup address information: Name or service not known".to_string(),
            retry_after_secs: None,
        },
        &mut remote,
    );

    // Auto-poke must stay enabled and queued work preserved.
    assert!(app.auto_poke_incomplete_todos);
    assert!(!app.queued_messages().is_empty());
    let pending = app
        .rate_limit_pending_message
        .as_ref()
        .expect("offline turn should be held for network recovery");
    // Promoted to auto_retry so the tick-based resume re-sends it.
    assert!(pending.auto_retry);
    assert_eq!(pending.retry_attempts, 0);
    assert!(app.rate_limit_reset.is_some());
    assert!(matches!(
        app.status,
        ProcessingStatus::WaitingForNetwork { .. }
    ));
    assert!(
        !app.display_messages()
            .iter()
            .any(|m| m.role == "system" && m.content.contains("Auto-poke stopped"))
    );
}

fn openai_oauth_route(model: &str) -> crate::provider::ModelRoute {
    crate::provider::ModelRoute {
        model: model.to_string(),
        provider: "OpenAI".to_string(),
        api_method: "openai-oauth".to_string(),
        available: true,
        detail: String::new(),
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
        cheapness: None,
    }
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
            id: 0,
            model: "claude-sonnet-4".to_string(),
            provider_name: Some("Anthropic".to_string()),
            error: None,
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
            id: 0,
            model: "claude-sonnet-4".to_string(),
            provider_name: None,
            error: Some("switch failed".to_string()),
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

#[test]
fn test_schedule_pending_remote_retry_respects_retry_limit() {
    let mut app = create_test_app();
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "retry me".to_string(),
        images: vec![],
        is_system: true,
        system_reminder: None,
        auto_retry: true,
        retry_attempts: App::AUTO_RETRY_MAX_ATTEMPTS,
        retry_at: None,
    });

    assert!(!app.schedule_pending_remote_retry("⚠ failed."));
    assert!(app.rate_limit_pending_message.is_none());
    assert!(app.rate_limit_reset.is_none());
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.role == "error" && m.content.contains("Auto-retry limit reached"))
    );
}

/// A provider guardrail refusal (e.g. Anthropic stop_reason "refusal") should
/// arm a one-keypress reroute offer to claude-opus-4-8, carrying the refused
/// payload so accepting the offer resends it on the stronger route.
#[test]
fn test_provider_guardrail_event_offers_opus_reroute_with_resend_payload() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_remote = true;
    app.remote_provider_name = Some("OpenAI".to_string());
    app.remote_provider_model = Some("gpt-5.5".to_string());
    app.remote_model_options = vec![
        openai_oauth_route("gpt-5.5"),
        claude_oauth_route("claude-sonnet-4"),
        claude_oauth_route("claude-opus-4-8"),
    ];
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "please help".to_string(),
        images: vec![],
        is_system: false,
        system_reminder: None,
        auto_retry: false,
        retry_attempts: 0,
        retry_at: None,
    });
    app.last_submitted_input = Some("please help".to_string());

    app.handle_server_event(
        crate::protocol::ServerEvent::ProviderGuardrail {
            stop_reason: Some("refusal".to_string()),
            message: "Provider guardrail stopped the response (stop_reason: refusal). The model declined to answer this request.".to_string(),
        },
        &mut remote,
    );

    let offer = app
        .pending_fallback_offer
        .as_ref()
        .expect("guardrail event should arm a reroute offer");
    assert_eq!(offer.selection.model, "claude-opus-4-8");
    assert_eq!(offer.selection.provider_label, "Anthropic");
    let resend = offer
        .remote_resend
        .as_ref()
        .expect("offer should capture the refused payload for resend");
    assert_eq!(resend.content, "please help");
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.role == "system" && m.content.contains("Reroute available")),
        "reroute offer message should be shown"
    );
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.role == "system" && m.content.contains("[guardrail]")),
        "guardrail notice itself should still be shown"
    );
}

/// The reroute offer must prefer native Anthropic auth over aggregator routes
/// that also expose claude-opus-4-8.
#[test]
fn test_guardrail_reroute_prefers_native_anthropic_route() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_remote = true;
    app.remote_provider_name = Some("OpenAI".to_string());
    app.remote_provider_model = Some("gpt-5.5".to_string());
    app.remote_model_options = vec![
        openai_oauth_route("gpt-5.5"),
        crate::provider::ModelRoute {
            model: "claude-opus-4-8".to_string(),
            provider: "OpenRouter".to_string(),
            api_method: "openrouter".to_string(),
            available: true,
            detail: String::new(),
            cheapness: None,
        },
        claude_oauth_route("claude-opus-4-8"),
    ];

    app.handle_server_event(
        crate::protocol::ServerEvent::ProviderGuardrail {
            stop_reason: Some("refusal".to_string()),
            message: "refused".to_string(),
        },
        &mut remote,
    );

    let offer = app
        .pending_fallback_offer
        .as_ref()
        .expect("guardrail event should arm a reroute offer");
    assert_eq!(offer.selection.provider_label, "Anthropic");
    assert_eq!(offer.selection.api_method, "claude-oauth");
}

/// No reroute offer when the session is already on claude-opus-4-8: there is
/// nothing stronger to hop to, so only the guardrail notice should appear.
#[test]
fn test_guardrail_reroute_not_offered_when_already_on_opus() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_remote = true;
    app.remote_provider_name = Some("Anthropic".to_string());
    app.remote_provider_model = Some("claude-opus-4-8".to_string());
    app.remote_model_options = vec![
        claude_oauth_route("claude-opus-4-8"),
        claude_oauth_route("claude-sonnet-4"),
    ];

    app.handle_server_event(
        crate::protocol::ServerEvent::ProviderGuardrail {
            stop_reason: Some("refusal".to_string()),
            message: "refused".to_string(),
        },
        &mut remote,
    );

    assert!(
        app.pending_fallback_offer.is_none(),
        "no reroute offer when already on the reroute target"
    );
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.role == "system" && m.content.contains("[guardrail]")),
        "guardrail notice should still be shown"
    );
}

/// No reroute offer when no claude-opus-4-8 route exists in the catalog.
#[test]
fn test_guardrail_reroute_not_offered_without_opus_route() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_remote = true;
    app.remote_provider_name = Some("OpenAI".to_string());
    app.remote_provider_model = Some("gpt-5.5".to_string());
    app.remote_model_options = vec![openai_oauth_route("gpt-5.5"), openai_oauth_route("gpt-5.4")];

    app.handle_server_event(
        crate::protocol::ServerEvent::ProviderGuardrail {
            stop_reason: Some("refusal".to_string()),
            message: "refused".to_string(),
        },
        &mut remote,
    );

    assert!(app.pending_fallback_offer.is_none());
}

#[test]
fn test_info_widget_data_includes_connection_type() {
    let mut app = create_test_app();
    app.connection_type = Some("https".to_string());
    let data = crate::tui::TuiState::info_widget_data(&app);
    assert_eq!(data.connection_type.as_deref(), Some("https"));
}

#[test]
fn test_remote_tui_state_prefers_cached_model_during_brief_connecting_phase() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("create temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let session_id = "session_otter_123";
    let mut session = crate::session::Session::create_with_id(
        session_id.to_string(),
        None,
        Some("remote cached model".to_string()),
    );
    session.model = Some("gpt-5.4".to_string());
    session.save().expect("save remote session");

    let app = App::new_for_remote(Some(session_id.to_string()));

    assert_eq!(crate::tui::TuiState::provider_model(&app), "gpt-5.4");
    assert_eq!(crate::tui::TuiState::provider_name(&app), "openai");
    assert_eq!(
        crate::tui::TuiState::session_display_name(&app).as_deref(),
        Some("otter")
    );

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_remote_tui_state_falls_back_to_cached_model_after_startup_phase_clears() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("create temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let session_id = "session_otter_124";
    let mut session = crate::session::Session::create_with_id(
        session_id.to_string(),
        None,
        Some("remote cached model".to_string()),
    );
    session.model = Some("gpt-5.4".to_string());
    session.save().expect("save remote session");

    let mut app = App::new_for_remote(Some(session_id.to_string()));
    app.clear_remote_startup_phase();

    assert_eq!(crate::tui::TuiState::provider_model(&app), "gpt-5.4");
    assert_eq!(crate::tui::TuiState::provider_name(&app), "openai");

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_new_for_remote_uses_startup_stub_without_loading_full_transcript() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("create temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let session_id = "session_otter_stub_125";
    let mut session = crate::session::Session::create_with_id(
        session_id.to_string(),
        None,
        Some("remote cached model".to_string()),
    );
    session.model = Some("gpt-5.4".to_string());
    session.append_stored_message(crate::session::StoredMessage {
        id: "msg-startup-stub".to_string(),
        role: crate::message::Role::User,
        content: vec![crate::message::ContentBlock::Text {
            text: "hello from persisted history".to_string(),
            cache_control: None,
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    });
    session.save().expect("save remote session");

    let app = App::new_for_remote(Some(session_id.to_string()));
    assert_eq!(app.session_id(), session_id);
    assert_eq!(app.display_messages().len(), 1);
    assert_eq!(
        app.display_messages()[0].content,
        "hello from persisted history"
    );
    // The remote client renders persisted history into `display_messages`,
    // then calls `strip_transcript_for_remote_client()` to release the backing
    // transcript (the server is the source of truth for the live transcript).
    // So the stripped `session.messages` is expected to be empty here.
    assert_eq!(app.session.messages.len(), 0);
    assert_eq!(app.remote_session_id.as_deref(), Some(session_id));
    assert_eq!(crate::tui::TuiState::provider_model(&app), "gpt-5.4");

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_remote_tui_state_shows_connected_after_startup_phase_clears_without_model() {
    let mut app = App::new_for_remote(None);
    app.remote_session_id = Some("session_connected_123".to_string());
    app.clear_remote_startup_phase();

    assert_eq!(crate::tui::TuiState::provider_model(&app), "connected");
    assert_eq!(crate::tui::TuiState::provider_name(&app), "");
}

#[test]
fn test_remote_tui_state_hides_brief_connecting_phase_without_cached_model() {
    let _guard = crate::storage::lock_test_env();
    let prev_model = std::env::var_os("JCODE_MODEL");
    let prev_provider = std::env::var_os("JCODE_PROVIDER");
    crate::env::set_var("JCODE_MODEL", "unknown");
    crate::env::remove_var("JCODE_PROVIDER");

    let app = App::new_for_remote(None);

    assert_eq!(
        crate::tui::TuiState::provider_model(&app),
        "connecting to server…"
    );
    assert_eq!(crate::tui::TuiState::provider_name(&app), "");

    if let Some(prev_model) = prev_model {
        crate::env::set_var("JCODE_MODEL", prev_model);
    } else {
        crate::env::remove_var("JCODE_MODEL");
    }
    if let Some(prev_provider) = prev_provider {
        crate::env::set_var("JCODE_PROVIDER", prev_provider);
    } else {
        crate::env::remove_var("JCODE_PROVIDER");
    }
}

#[test]
fn test_remote_tui_state_prefers_configured_model_during_brief_connecting_phase() {
    let _guard = crate::storage::lock_test_env();
    let prev_model = std::env::var_os("JCODE_MODEL");
    let prev_provider = std::env::var_os("JCODE_PROVIDER");
    crate::env::set_var("JCODE_MODEL", "gpt-5.4");
    crate::env::set_var("JCODE_PROVIDER", "openai");

    let app = App::new_for_remote(None);

    assert_eq!(crate::tui::TuiState::provider_model(&app), "gpt-5.4");
    assert_eq!(crate::tui::TuiState::provider_name(&app), "openai");

    if let Some(prev_model) = prev_model {
        crate::env::set_var("JCODE_MODEL", prev_model);
    } else {
        crate::env::remove_var("JCODE_MODEL");
    }
    if let Some(prev_provider) = prev_provider {
        crate::env::set_var("JCODE_PROVIDER", prev_provider);
    } else {
        crate::env::remove_var("JCODE_PROVIDER");
    }
}

#[test]
fn test_remote_tui_state_shows_starting_server_phase_in_header() {
    let mut app = App::new_for_remote(None);
    app.set_server_spawning();

    assert_eq!(
        crate::tui::TuiState::provider_model(&app),
        "starting server…"
    );
}

#[test]
fn test_remote_tui_state_shows_loading_session_phase_in_header() {
    let mut app = App::new_for_remote(None);
    app.set_remote_startup_phase(crate::tui::app::RemoteStartupPhase::LoadingSession);

    assert_eq!(
        crate::tui::TuiState::provider_model(&app),
        "loading session…"
    );
}

#[test]
fn test_remote_tui_state_shows_startup_elapsed_in_header() {
    let mut app = App::new_for_remote(None);
    app.set_remote_startup_phase(crate::tui::app::RemoteStartupPhase::Connecting);
    app.remote_startup_phase_started =
        Some(std::time::Instant::now() - std::time::Duration::from_secs(5));

    assert_eq!(
        crate::tui::TuiState::provider_model(&app),
        "connecting to server… 5s"
    );
}

#[test]
fn test_remote_startup_phase_does_not_require_duplicate_status_notice() {
    let mut app = App::new_for_remote(None);
    app.set_remote_startup_phase(crate::tui::app::RemoteStartupPhase::Connecting);

    assert_eq!(
        crate::tui::TuiState::provider_model(&app),
        "connecting to server…"
    );
    assert_eq!(app.status_notice(), None);

    app.set_remote_startup_phase(crate::tui::app::RemoteStartupPhase::LoadingSession);
    assert_eq!(
        crate::tui::TuiState::provider_model(&app),
        "loading session…"
    );
    assert_eq!(app.status_notice(), None);
}

#[test]
fn test_remote_tui_state_shows_reconnecting_phase_in_header() {
    let mut app = App::new_for_remote(None);
    app.set_remote_startup_phase(crate::tui::app::RemoteStartupPhase::Reconnecting { attempt: 3 });

    assert_eq!(
        crate::tui::TuiState::provider_model(&app),
        "reconnecting (3)…"
    );
}

#[test]
fn test_remote_header_keeps_known_model_during_brief_loading_session_phase() {
    // Routine bootstrap: the model is already known (session stub / config
    // hint), so a short LoadingSession phase must not flash "loading session…"
    // over it. That pre-settle churn is exactly what made spawns look unstable.
    let mut app = App::new_for_remote(None);
    app.session.model = Some("claude-fable-5".to_string());
    app.set_remote_startup_phase(crate::tui::app::RemoteStartupPhase::LoadingSession);

    assert_eq!(crate::tui::TuiState::provider_model(&app), "claude-fable-5");

    // A genuinely stuck load still surfaces the phase label after the grace
    // period so the user can tell something is wrong.
    app.remote_startup_phase_started =
        Some(std::time::Instant::now() - std::time::Duration::from_secs(5));
    assert_eq!(
        crate::tui::TuiState::provider_model(&app),
        "loading session… 5s"
    );
}

#[test]
fn test_remote_effort_identity_falls_back_to_session_model_before_history() {
    // Before the server History payload lands, remote_provider_name/model are
    // None, but the session stub already knows the model. Effort cycling must
    // work off that hint instead of reporting "not available".
    let mut app = App::new_for_remote(None);
    app.session.model = Some("claude-fable-5".to_string());
    assert!(app.remote_provider_name.is_none());
    assert!(app.remote_provider_model.is_none());

    let (provider, model) = app.remote_effort_identity();
    assert_eq!(model.as_deref(), Some("claude-fable-5"));
    let efforts =
        crate::tui::app::inferred_reasoning_efforts(provider.as_deref(), model.as_deref());
    assert!(
        !efforts.is_empty(),
        "pre-History effort cycling must resolve levels from the model hint"
    );
    assert!(efforts.contains(&"xhigh"));

    // Server-reported values still win once they arrive.
    app.remote_provider_name = Some("openai".to_string());
    app.remote_provider_model = Some("gpt-5.3-codex".to_string());
    let (provider, model) = app.remote_effort_identity();
    assert_eq!(provider.as_deref(), Some("openai"));
    assert_eq!(model.as_deref(), Some("gpt-5.3-codex"));
}

#[test]
fn test_openai_compatible_login_preserves_profile_for_runtime_activation() {
    let mut app = create_test_app();

    app.start_login_provider(crate::provider_catalog::ZAI_LOGIN_PROVIDER);

    match app.pending_login {
        Some(crate::tui::app::PendingLogin::ApiKeyProfile {
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
fn test_tui_login_providers_have_real_tui_handlers() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let _guard = runtime.enter();
    let unsupported_needles = [
        "CLI-only",
        "only available from the CLI",
        "currently CLI-only",
    ];

    for provider in crate::provider_catalog::tui_login_providers() {
        let mut app = create_test_app();

        app.start_login_provider(provider);

        let rendered_messages = app
            .display_messages()
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        for needle in unsupported_needles {
            assert!(
                !rendered_messages.contains(needle),
                "TUI-visible login provider `{}` emitted unsupported surface message `{}`: {}",
                provider.id,
                needle,
                rendered_messages
            );
        }
    }
}

#[test]
fn test_info_widget_remote_openai_uses_remote_provider_for_usage_and_context() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_provider_name = Some("OpenAI".to_string());
    app.remote_provider_model = Some("gpt-5.4".to_string());
    app.remote_resolved_credential = Some(jcode_provider_core::ResolvedCredential::Oauth);
    app.update_context_limit_for_model("gpt-5.4");

    let data = crate::tui::TuiState::info_widget_data(&app);

    assert_eq!(data.provider_name.as_deref(), Some("OpenAI"));
    assert_eq!(data.model.as_deref(), Some("gpt-5.4"));
    assert_eq!(data.context_limit, Some(1_000_000));
    assert_eq!(
        data.auth_method,
        crate::tui::info_widget::AuthMethod::OpenAIOAuth
    );
    assert_eq!(
        data.usage_info.as_ref().map(|info| info.provider),
        Some(crate::tui::info_widget::UsageProvider::OpenAI)
    );
}

#[test]
fn test_info_widget_remote_model_falls_back_to_model_provider_detection() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_provider_model = Some("gpt-5.4".to_string());
    app.update_context_limit_for_model("gpt-5.4");

    let data = crate::tui::TuiState::info_widget_data(&app);

    assert_eq!(data.context_limit, Some(1_000_000));
    assert_eq!(
        data.auth_method,
        crate::tui::info_widget::AuthMethod::Unknown
    );
    assert!(
        data.usage_info.is_none(),
        "provider/model detection alone must not guess subscription billing"
    );
}

#[test]
fn test_info_widget_remote_opencode_shows_cost_based_usage() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_provider_name = Some("opencode".to_string());
    app.remote_provider_model = Some("qwen3-coder".to_string());
    app.token_accounting.total_input_tokens = 12_000;
    app.token_accounting.total_output_tokens = 3_400;

    let data = crate::tui::TuiState::info_widget_data(&app);

    assert_eq!(data.provider_name.as_deref(), Some("opencode"));
    let usage = data.usage_info.as_ref().expect("opencode usage info");
    assert_eq!(
        usage.provider,
        crate::tui::info_widget::UsageProvider::CostBased
    );
    assert!(usage.available);
    assert_eq!(usage.input_tokens, 12_000);
    assert_eq!(usage.output_tokens, 3_400);
}

#[test]
fn test_info_widget_remote_anthropic_api_key_shows_cost_based_usage() {
    // Remote Anthropic sessions billed via API key (server resolves
    // ResolvedCredential::ApiKey) should display cost-based usage instead of
    // subscription bars, mirroring local behavior. OAuth subscription sessions
    // (server resolves ResolvedCredential::Oauth) keep the subscription usage
    // provider.
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_provider_name = Some("Claude".to_string());
    app.remote_provider_model = Some("claude-sonnet-4-20250514".to_string());
    app.remote_resolved_credential = Some(jcode_provider_core::ResolvedCredential::ApiKey);
    app.token_accounting.total_input_tokens = 12_000;
    app.token_accounting.total_output_tokens = 3_400;

    let data = crate::tui::TuiState::info_widget_data(&app);
    assert_eq!(
        data.auth_method,
        crate::tui::info_widget::AuthMethod::AnthropicApiKey
    );
    let usage = data
        .usage_info
        .as_ref()
        .expect("remote anthropic api-key usage info");
    assert_eq!(
        usage.provider,
        crate::tui::info_widget::UsageProvider::CostBased
    );
    assert_eq!(usage.input_tokens, 12_000);
    assert_eq!(usage.output_tokens, 3_400);

    // OAuth subscription keeps subscription bars; the server now reports the
    // resolved credential directly, so the widget reflects AnthropicOAuth.
    app.remote_resolved_credential = Some(jcode_provider_core::ResolvedCredential::Oauth);
    let data = crate::tui::TuiState::info_widget_data(&app);
    assert_eq!(
        data.auth_method,
        crate::tui::info_widget::AuthMethod::AnthropicOAuth
    );
    assert_eq!(
        data.usage_info.as_ref().map(|info| info.provider),
        Some(crate::tui::info_widget::UsageProvider::Anthropic)
    );
}

#[test]
fn test_info_widget_remote_openai_billing_follows_resolved_credential() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_provider_name = Some("OpenAI".to_string());
    app.remote_provider_model = Some("gpt-5.4".to_string());
    app.token_accounting.total_input_tokens = 12_000;
    app.token_accounting.total_output_tokens = 3_400;

    app.remote_resolved_credential = Some(jcode_provider_core::ResolvedCredential::ApiKey);
    let data = crate::tui::TuiState::info_widget_data(&app);
    assert_eq!(
        data.auth_method,
        crate::tui::info_widget::AuthMethod::OpenAIApiKey
    );
    let usage = data.usage_info.as_ref().expect("OpenAI API cost info");
    assert_eq!(
        usage.provider,
        crate::tui::info_widget::UsageProvider::CostBased
    );
    assert_eq!(usage.input_tokens, 12_000);
    assert_eq!(usage.output_tokens, 3_400);

    app.remote_resolved_credential = Some(jcode_provider_core::ResolvedCredential::Oauth);
    let data = crate::tui::TuiState::info_widget_data(&app);
    assert_eq!(
        data.auth_method,
        crate::tui::info_widget::AuthMethod::OpenAIOAuth
    );
    assert_eq!(
        data.usage_info.as_ref().map(|usage| usage.provider),
        Some(crate::tui::info_widget::UsageProvider::OpenAI)
    );

    app.remote_resolved_credential = None;
    app.session.route_api_method = None;
    let data = crate::tui::TuiState::info_widget_data(&app);
    assert_eq!(
        data.auth_method,
        crate::tui::info_widget::AuthMethod::Unknown
    );
    assert!(data.usage_info.is_none());
}

#[test]
fn test_info_widget_remote_openai_uses_explicit_route_when_credential_is_missing() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_provider_name = Some("OpenAI".to_string());
    app.remote_provider_model = Some("gpt-5.4".to_string());
    app.remote_resolved_credential = None;

    app.session.route_api_method = Some("openai-api-key".to_string());
    let data = crate::tui::TuiState::info_widget_data(&app);
    assert_eq!(
        data.auth_method,
        crate::tui::info_widget::AuthMethod::OpenAIApiKey
    );
    assert_eq!(
        data.usage_info.as_ref().map(|usage| usage.provider),
        Some(crate::tui::info_widget::UsageProvider::CostBased)
    );

    app.session.route_api_method = Some("openai-oauth".to_string());
    let data = crate::tui::TuiState::info_widget_data(&app);
    assert_eq!(
        data.auth_method,
        crate::tui::info_widget::AuthMethod::OpenAIOAuth
    );
    assert_eq!(
        data.usage_info.as_ref().map(|usage| usage.provider),
        Some(crate::tui::info_widget::UsageProvider::OpenAI)
    );
}

#[test]
fn test_info_widget_local_direct_api_runtime_shows_cost_based_usage() {
    let _guard = crate::storage::lock_test_env();
    let tracked_env = [
        "JCODE_RUNTIME_PROVIDER",
        "JCODE_OPENROUTER_ALLOW_NO_AUTH",
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_PROVIDER_FEATURES",
        "JCODE_OPENROUTER_TRANSPORT_STATE",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_NAMED_PROVIDER_PROFILE",
        "JCODE_PROVIDER_PROFILE_ACTIVE",
        "JCODE_PROVIDER_PROFILE_NAME",
    ];
    let saved_env = tracked_env
        .iter()
        .map(|&key| (key, std::env::var_os(key)))
        .collect::<Vec<_>>();
    for &key in &tracked_env {
        crate::env::remove_var(key);
    }

    let cases = [
        (
            "claude-api",
            "anthropic",
            "claude-sonnet-4-6",
            crate::tui::info_widget::AuthMethod::AnthropicApiKey,
        ),
        (
            "openai-api",
            "openai",
            "gpt-5.4",
            crate::tui::info_widget::AuthMethod::OpenAIApiKey,
        ),
        (
            "openrouter",
            "openrouter",
            "anthropic/claude-sonnet-4",
            crate::tui::info_widget::AuthMethod::OpenRouterApiKey,
        ),
        (
            "openai-compatible",
            "openrouter",
            "direct-compatible-model",
            crate::tui::info_widget::AuthMethod::ApiKey,
        ),
        (
            "openai-compatible",
            "cerebras",
            "gpt-oss-120b",
            crate::tui::info_widget::AuthMethod::ApiKey,
        ),
        (
            "bedrock",
            "bedrock",
            "anthropic.claude-3-5-sonnet-20241022-v2:0",
            crate::tui::info_widget::AuthMethod::ApiKey,
        ),
    ];

    for (runtime_provider, provider_name, model, expected_auth) in cases {
        crate::env::set_var("JCODE_RUNTIME_PROVIDER", runtime_provider);
        crate::env::remove_var("JCODE_OPENROUTER_ALLOW_NO_AUTH");
        crate::auth::AuthStatus::invalidate_cache();

        let mut app = create_named_provider_test_app(provider_name, model);
        app.streaming.streaming_input_tokens = 1_000;
        app.streaming.streaming_output_tokens = 1_000;
        app.token_accounting.total_input_tokens = 12_000;
        app.token_accounting.total_output_tokens = 3_400;
        app.update_cost_impl();

        assert!(
            app.cost.total_cost > 0.0,
            "{runtime_provider} should accrue token cost"
        );

        let data = crate::tui::TuiState::info_widget_data(&app);
        assert_eq!(data.auth_method, expected_auth);
        let usage = data
            .usage_info
            .as_ref()
            .expect("direct API runtime usage info");
        assert_eq!(
            usage.provider,
            crate::tui::info_widget::UsageProvider::CostBased
        );
        assert_eq!(usage.input_tokens, 12_000);
        assert_eq!(usage.output_tokens, 3_400);
        assert!(usage.total_cost > 0.0);
    }

    crate::env::set_var("JCODE_RUNTIME_PROVIDER", "jcode");
    crate::env::remove_var("JCODE_OPENROUTER_ALLOW_NO_AUTH");
    let mut app = create_named_provider_test_app("openrouter", "subscription-model");
    app.streaming.streaming_input_tokens = 1_000;
    app.streaming.streaming_output_tokens = 1_000;
    app.token_accounting.total_input_tokens = 12_000;
    app.token_accounting.total_output_tokens = 3_400;
    app.update_cost_impl();
    assert_eq!(app.cost.total_cost, 0.0);

    let data = crate::tui::TuiState::info_widget_data(&app);
    assert_eq!(
        data.auth_method,
        crate::tui::info_widget::AuthMethod::Unknown
    );
    assert!(data.usage_info.is_none());

    crate::env::set_var("JCODE_RUNTIME_PROVIDER", "openai-compatible");
    crate::env::set_var("JCODE_OPENROUTER_ALLOW_NO_AUTH", "1");
    let mut app = create_named_provider_test_app("openrouter", "local-model");
    app.streaming.streaming_input_tokens = 1_000;
    app.streaming.streaming_output_tokens = 1_000;
    app.token_accounting.total_input_tokens = 12_000;
    app.token_accounting.total_output_tokens = 3_400;
    app.update_cost_impl();
    assert_eq!(app.cost.total_cost, 0.0);

    let data = crate::tui::TuiState::info_widget_data(&app);
    assert_eq!(
        data.auth_method,
        crate::tui::info_widget::AuthMethod::Unknown
    );
    assert!(data.usage_info.is_none());

    for (key, value) in saved_env {
        if let Some(value) = value {
            crate::env::set_var(key, value);
        } else {
            crate::env::remove_var(key);
        }
    }
    crate::auth::AuthStatus::invalidate_cache();
}

#[test]
fn test_anthropic_api_cost_accounts_for_split_cache_tokens() {
    // Anthropic reports usage with *split* accounting: `input_tokens` already
    // excludes cache-read and cache-creation tokens. The cost figure must
    //   - bill fresh input at the input rate,
    //   - bill cache-read tokens at the (cheaper) cache-read rate WITHOUT also
    //     subtracting them from the fresh input (double subtraction), and
    //   - bill cache-creation (cache-write) tokens, which Anthropic charges at a
    //     premium over the input rate.
    let _guard = crate::storage::lock_test_env();
    let saved_runtime = std::env::var_os("JCODE_RUNTIME_PROVIDER");
    crate::env::set_var("JCODE_RUNTIME_PROVIDER", "claude-api");
    crate::auth::AuthStatus::invalidate_cache();

    // claude-sonnet-4-6 API pricing: input $3/Mtok, output $15/Mtok,
    // cache-read $0.30/Mtok. Cache-write (1h TTL) is billed at 2x input = $6/Mtok.
    let mut app = create_named_provider_test_app("anthropic", "claude-sonnet-4-6");
    crate::provider::anthropic::set_cache_ttl_1h(true);

    // A representative cold turn: most of the prompt is freshly written to cache,
    // a little is read back, and only a small uncached remainder is fresh input.
    app.streaming.streaming_input_tokens = 1_000; // uncached fresh input
    app.streaming.streaming_cache_read_tokens = Some(40_000); // served from cache
    app.streaming.streaming_cache_creation_tokens = Some(100_000); // written to cache (premium)
    app.streaming.streaming_output_tokens = 2_000;
    app.update_cost_impl();

    // Expected:
    //   fresh input:    1_000  * $3   / 1e6 = $0.003
    //   output:         2_000  * $15  / 1e6 = $0.030
    //   cache read:    40_000  * $0.3 / 1e6 = $0.012
    //   cache write:  100_000  * $6   / 1e6 = $0.600
    //   total                                = $0.645
    let expected = 0.003 + 0.030 + 0.012 + 0.600;
    assert!(
        (app.cost.total_cost - expected).abs() < 1e-4,
        "anthropic split-accounting cost should be ~${expected:.4}, got ${:.4}",
        app.cost.total_cost
    );

    if let Some(value) = saved_runtime {
        crate::env::set_var("JCODE_RUNTIME_PROVIDER", value);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_PROVIDER");
    }
    crate::auth::AuthStatus::invalidate_cache();
}

#[test]
fn test_remote_anthropic_api_key_accrues_cost_from_token_usage() {
    // The default interactive TUI is a remote client: it receives per-call
    // ServerEvent::TokenUsage but never runs the local finish_turn cost path.
    // Anthropic API-key sessions must still accrue a dollar cost from those
    // events (the server reports tokens, not cost), and OAuth subscription
    // sessions must stay at $0.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_provider_name = Some("Claude".to_string());
    app.remote_provider_model = Some("claude-sonnet-4-6".to_string());
    app.remote_resolved_credential = Some(jcode_provider_core::ResolvedCredential::ApiKey);
    crate::provider::anthropic::set_cache_ttl_1h(true);

    // One completed call with split-accounting cache telemetry.
    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 1_000,
            output: 2_000,
            cache_read_input: Some(40_000),
            cache_creation_input: Some(100_000),
        },
        &mut remote,
    );

    // Same expected math as the local split-accounting test:
    //   input 1_000 * $3 + output 2_000 * $15 + read 40_000 * $0.3
    //   + write 100_000 * ($3 * 2x) = $0.645
    let expected = 0.003 + 0.030 + 0.012 + 0.600;
    assert!(
        (app.cost.total_cost - expected).abs() < 1e-4,
        "remote anthropic api-key cost should be ~${expected:.4}, got ${:.4}",
        app.cost.total_cost
    );
    assert_eq!(app.token_accounting.total_input_tokens, 1_000);
    assert_eq!(app.token_accounting.total_output_tokens, 2_000);

    // OAuth subscription sessions are not metered per token; cost stays $0.
    let mut oauth_app = create_test_app();
    oauth_app.is_remote = true;
    oauth_app.remote_provider_name = Some("Claude".to_string());
    oauth_app.remote_provider_model = Some("claude-sonnet-4-6".to_string());
    oauth_app.remote_resolved_credential = Some(jcode_provider_core::ResolvedCredential::Oauth);
    oauth_app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 1_000,
            output: 2_000,
            cache_read_input: Some(40_000),
            cache_creation_input: Some(100_000),
        },
        &mut remote,
    );
    assert_eq!(oauth_app.cost.total_cost, 0.0);
    assert_eq!(oauth_app.token_accounting.total_input_tokens, 1_000);
}

#[test]
fn test_resumed_session_seeds_cost_from_history_token_totals() {
    // Reopening an older session restores token totals from history but never
    // ran the live per-call cost path, so the cost widget showed $0. The resume
    // path must price the restored totals once to seed total_cost.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();

    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_provider_name = Some("Claude".to_string());
    app.remote_provider_model = Some("claude-sonnet-4-6".to_string());
    app.remote_resolved_credential = Some(jcode_provider_core::ResolvedCredential::ApiKey);
    crate::provider::anthropic::set_cache_ttl_1h(true);

    let totals = crate::protocol::TokenUsageTotals {
        messages_with_token_usage: 3,
        input_tokens: 1_000,
        output_tokens: 2_000,
        cache_reported_input_tokens: 1_000,
        cache_read_input_tokens: 40_000,
        cache_creation_input_tokens: 100_000,
    };
    app.seed_cost_from_history_totals(&totals);

    // Same split-accounting math as the live-call test above.
    let expected = 0.003 + 0.030 + 0.012 + 0.600;
    assert!(
        (app.cost.total_cost - expected).abs() < 1e-4,
        "resumed session cost should be seeded to ~${expected:.4}, got ${:.4}",
        app.cost.total_cost
    );

    // Idempotent: a repeated history snapshot must not double the cost.
    app.seed_cost_from_history_totals(&totals);
    assert!(
        (app.cost.total_cost - expected).abs() < 1e-4,
        "re-seeding must overwrite (not accrue), got ${:.4}",
        app.cost.total_cost
    );

    // OAuth subscription sessions are not metered per token; cost stays $0.
    let mut oauth_app = create_test_app();
    oauth_app.is_remote = true;
    oauth_app.remote_provider_name = Some("Claude".to_string());
    oauth_app.remote_provider_model = Some("claude-sonnet-4-6".to_string());
    oauth_app.remote_resolved_credential = Some(jcode_provider_core::ResolvedCredential::Oauth);
    oauth_app.seed_cost_from_history_totals(&totals);
    assert_eq!(oauth_app.cost.total_cost, 0.0);
}

#[test]
fn test_remote_fast_mode_tier_bills_premium_rates_and_reprices_on_toggle() {
    // `/fast on` (priority tier) bills premium per-token rates on Opus 4.6
    // ($30/$150 vs $5/$25). The pricing memo key includes the tier so toggling
    // fast mode mid-session re-resolves prices instead of reusing stale ones.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_provider_name = Some("Claude".to_string());
    app.remote_provider_model = Some("claude-opus-4-6".to_string());
    app.remote_resolved_credential = Some(jcode_provider_core::ResolvedCredential::ApiKey);

    // Each TokenUsage below simulates a separate completed API call, so reset
    // the per-call usage bookkeeping between them (a real session does this at
    // call start).
    let reset_call_state = |app: &mut App| {
        app.kv_cache.current_api_usage_recorded = false;
        app.streaming.streaming_input_tokens = 0;
        app.streaming.streaming_output_tokens = 0;
        app.streaming.streaming_cache_read_tokens = None;
        app.streaming.streaming_cache_creation_tokens = None;
    };

    // Standard tier first: 1k in / 1k out = $0.005 + $0.025 = $0.030.
    app.remote_service_tier = None;
    reset_call_state(&mut app);
    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 1_000,
            output: 1_000,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );
    let standard_cost = app.cost.total_cost;
    assert!(
        (standard_cost - 0.030).abs() < 1e-4,
        "standard-tier cost should be ~$0.030, got ${standard_cost:.4}"
    );

    // Fast mode on: same usage now bills $0.030 + $0.150 = $0.180.
    app.remote_service_tier = Some("auto".to_string());
    reset_call_state(&mut app);
    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 1_000,
            output: 1_000,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );
    let fast_call_cost = app.cost.total_cost - standard_cost;
    assert!(
        (fast_call_cost - 0.180).abs() < 1e-4,
        "fast-mode call cost should be ~$0.180, got ${fast_call_cost:.4}"
    );

    // Fast mode off again: pricing drops back to standard rates.
    app.remote_service_tier = None;
    reset_call_state(&mut app);
    let before = app.cost.total_cost;
    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 1_000,
            output: 1_000,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );
    let off_call_cost = app.cost.total_cost - before;
    assert!(
        (off_call_cost - 0.030).abs() < 1e-4,
        "post-toggle standard cost should be ~$0.030, got ${off_call_cost:.4}"
    );
}

#[test]
fn test_info_widget_local_gemini_shows_oauth_auth_method() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("create temp dir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let path = crate::auth::gemini::tokens_path().expect("gemini tokens path");
    crate::storage::write_json_secret(
        &path,
        &serde_json::json!({
            "access_token": "at-123",
            "refresh_token": "rt-456",
            "expires_at": 4102444800000i64,
            "email": "user@example.com"
        }),
    )
    .expect("write gemini tokens");
    crate::auth::AuthStatus::invalidate_cache();

    let app = create_gemini_test_app();
    let data = crate::tui::TuiState::info_widget_data(&app);

    assert_eq!(data.provider_name.as_deref(), Some("gemini"));
    assert_eq!(data.model.as_deref(), Some("gemini-2.5-pro"));
    assert_eq!(
        data.auth_method,
        crate::tui::info_widget::AuthMethod::GeminiOAuth
    );
    assert!(data.usage_info.is_none());

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
    crate::auth::AuthStatus::invalidate_cache();
}

#[test]
fn test_debug_command_message_respects_queue_mode() {
    let mut app = create_test_app();

    // Test 1: When not processing, should submit directly
    let initial_session_messages = app.session.messages.len();
    app.is_processing = false;
    let result = app.handle_debug_command("message:hello");
    assert!(
        result.starts_with("OK: submitted message"),
        "Expected submitted, got: {}",
        result
    );
    // The message should be processed for display/session storage while local
    // provider messages are not retained in `app.messages`.
    assert!(app.pending_turn);
    assert_eq!(app.messages.len(), 0);
    assert_eq!(app.display_messages.len(), 1);
    assert_eq!(app.session.messages.len(), initial_session_messages + 1);
    let submitted_message = app
        .session
        .messages
        .last()
        .expect("submitted debug message should be stored");
    assert_eq!(submitted_message.role, crate::message::Role::User);
    assert_eq!(submitted_message.display_role, None);
    assert_eq!(submitted_message.content_preview(), "hello");

    // Reset for next test
    app.pending_turn = false;
    app.messages.clear();
    app.display_messages.clear();
    app.session.messages.clear();

    // Test 2: When processing with queue_mode=true, should queue
    app.is_processing = true;
    app.queue_mode = true;
    let result = app.handle_debug_command("message:queued_msg");
    assert!(
        result.contains("queued"),
        "Expected queued, got: {}",
        result
    );
    assert_eq!(app.queued_count(), 1);
    assert_eq!(app.queued_messages()[0], "queued_msg");

    // Test 3: When processing with queue_mode=false, should interleave
    app.queued_messages.clear();
    app.queue_mode = false;
    let result = app.handle_debug_command("message:interleave_msg");
    assert!(
        result.contains("interleave"),
        "Expected interleave, got: {}",
        result
    );
    assert_eq!(app.interleave_message.as_deref(), Some("interleave_msg"));
}

#[test]
fn test_debug_command_side_panel_latency_bench_reports_immediate_redraw() {
    // run_side_panel_latency_bench mutates process-global mermaid/markdown
    // state (diagram-mode override, ACTIVE_DIAGRAMS snapshot/restore), so
    // serialize with the other diagram-mutating tests.
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    let result = app.handle_debug_command(
        r#"side-panel-latency:{"iterations":8,"warmup_iterations":2,"include_samples":false}"#,
    );
    let value: serde_json::Value =
        serde_json::from_str(&result).expect("side-panel latency bench should return JSON");

    assert_eq!(value.get("ok").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(
        value["summary"]["scroll_only_count"].as_u64(),
        Some(0),
        "side-panel latency bench should observe immediate redraw events"
    );
    assert_eq!(
        value["summary"]["unchanged_scroll_count"].as_u64(),
        Some(0),
        "each injected event should change effective side-pane scroll"
    );
    assert!(
        value["summary"]["latency_ms"]["p95"]
            .as_f64()
            .unwrap_or_default()
            < 16.0,
        "headless side-panel p95 latency should stay within a 60fps frame budget: {}",
        result
    );
}

#[test]
fn test_debug_command_mermaid_flicker_bench_returns_json() {
    let mut app = create_test_app();
    let result = app.handle_debug_command("mermaid:flicker-bench 8");
    let value: serde_json::Value =
        serde_json::from_str(&result).expect("flicker bench should return JSON");

    assert_eq!(value["steps"].as_u64(), Some(8));
    assert!(
        value
            .get("protocol_supported")
            .and_then(|v| v.as_bool())
            .is_some(),
        "expected protocol_supported bool in result: {}",
        result
    );
    assert!(
        value.get("deltas").is_some(),
        "expected delta counters: {}",
        result
    );
}

#[test]
fn test_remote_transcript_send_uses_remote_submission_path() {
    let mut app = create_test_app();
    app.is_remote = true;
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    rt.block_on(async {
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        super::remote::apply_remote_transcript_event(
            &mut app,
            &mut remote,
            "dictated hello".to_string(),
            crate::protocol::TranscriptMode::Send,
        )
        .await
    })
    .expect("remote transcript send should succeed");

    let last = app
        .display_messages()
        .last()
        .expect("user message displayed");
    assert_eq!(last.role, "user");
    assert_eq!(last.content, "[transcription] dictated hello");
    assert!(
        app.is_processing,
        "remote send should enter processing state"
    );
    assert!(matches!(app.status, ProcessingStatus::Sending));
    assert!(
        app.current_message_id.is_some(),
        "remote request id should be assigned"
    );
    assert!(
        app.last_stream_activity.is_some(),
        "remote send should start stall timer from a real send"
    );
    assert!(
        !app.pending_turn,
        "remote transcript send must not use local pending_turn path"
    );
    assert!(
        app.input.is_empty(),
        "submitted transcript should clear input"
    );
    assert!(
        app.rate_limit_pending_message.is_some(),
        "remote send should populate retry state for the in-flight request"
    );
}

#[test]
fn test_remote_review_shows_processing_until_split_response() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.input = "/review".to_string();
    app.cursor_pos = app.input.len();

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
        .expect("/review should launch split request");

    assert!(
        app.is_processing,
        "review launch should show client processing state"
    );
    assert!(matches!(app.status, ProcessingStatus::Sending));
    assert!(app.current_message_id.is_none());
    assert_eq!(app.status_notice(), Some("Review launching".to_string()));
    assert!(app.pending_split_startup_message.is_some());
    assert_eq!(app.pending_split_label.as_deref(), Some("Review"));
    assert!(!app.pending_split_request);

    app.handle_server_event(
        crate::protocol::ServerEvent::SplitResponse {
            id: 1,
            new_session_id: "session_review_child".to_string(),
            new_session_name: "review_child".to_string(),
        },
        &mut remote,
    );

    assert!(
        !app.is_processing,
        "split response should clear transient launch state"
    );
    assert!(matches!(app.status, ProcessingStatus::Idle));
    assert!(app.processing_started.is_none());
    assert!(app.pending_split_startup_message.is_none());
    assert!(app.pending_split_label.is_none());
}

#[test]
fn test_remote_super_space_routes_next_prompt_to_new_session() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.is_remote = true;
        app.input = "hello from split".to_string();
        app.cursor_pos = app.input.len();

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        rt.block_on(app.handle_remote_key(KeyCode::Char(' '), KeyModifiers::SUPER, &mut remote))
            .expect("Super+Space should arm routing");
        assert!(app.route_next_prompt_to_new_session);

        app.is_processing = true;
        app.status = ProcessingStatus::Streaming;
        app.processing_started = Some(std::time::Instant::now());
        let active_started = app.processing_started;

        rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
            .expect("armed prompt should launch split request immediately");

        assert!(!app.route_next_prompt_to_new_session);
        assert!(app.pending_split_prompt.is_some());
        assert_eq!(app.pending_split_label.as_deref(), Some("Prompt"));
        assert!(!app.pending_split_request);
        assert!(app.is_processing);
        assert!(matches!(app.status, ProcessingStatus::Streaming));
        assert_eq!(app.processing_started, active_started);
        assert!(app.current_message_id.is_none());

        app.handle_server_event(
            crate::protocol::ServerEvent::SplitResponse {
                id: 1,
                new_session_id: "session_prompt_child".to_string(),
                new_session_name: "prompt_child".to_string(),
            },
            &mut remote,
        );

        let restored = App::restore_input_for_reload("session_prompt_child")
            .expect("new prompt session should have startup submission saved");
        assert_eq!(restored.input, "hello from split");
        assert!(restored.submit_on_restore);
        assert!(restored.pending_images.is_empty());
        assert!(app.pending_split_prompt.is_none());
        assert!(app.pending_split_label.is_none());
    });
}

#[test]
fn test_remote_judge_shows_processing_until_split_response() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.input = "/judge".to_string();
    app.cursor_pos = app.input.len();

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
        .expect("/judge should launch split request");

    assert!(
        app.is_processing,
        "judge launch should show client processing state"
    );
    assert!(matches!(app.status, ProcessingStatus::Sending));
    assert!(app.current_message_id.is_none());
    assert_eq!(app.status_notice(), Some("Judge launching".to_string()));
    assert!(app.pending_split_startup_message.is_some());
    assert_eq!(app.pending_split_label.as_deref(), Some("Judge"));
    assert!(!app.pending_split_request);

    app.handle_server_event(
        crate::protocol::ServerEvent::SplitResponse {
            id: 1,
            new_session_id: "session_judge_child".to_string(),
            new_session_name: "judge_child".to_string(),
        },
        &mut remote,
    );

    assert!(
        !app.is_processing,
        "split response should clear transient launch state"
    );
    assert!(matches!(app.status, ProcessingStatus::Idle));
    assert!(app.processing_started.is_none());
    assert!(app.pending_split_startup_message.is_none());
    assert!(app.pending_split_label.is_none());
}

// ====================================================================

#[test]
fn test_externally_started_turn_adopts_processing_state_and_settles_on_done() {
    // A swarm wake / background-task wake / scheduled task can start a turn in
    // this session without this client sending a message. The client must show
    // the turn as in-progress (spinner) and settle it when the terminal Done
    // arrives, instead of staying visually idle while text streams in.
    let mut app = create_test_app();
    app.is_remote = true;
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    assert!(!app.is_processing);
    assert!(app.current_message_id.is_none());

    app.handle_server_event(
        crate::protocol::ServerEvent::TextDelta {
            text: "Wake turn streaming text".to_string(),
        },
        &mut remote,
    );

    assert!(
        app.is_processing,
        "stream events while idle must adopt the externally started turn"
    );
    assert!(
        app.processing_started.is_some(),
        "adopted turn should start the elapsed/spinner clock"
    );
    assert!(
        matches!(app.status, ProcessingStatus::Streaming),
        "adopted turn should show streaming status, got {:?}",
        app.status
    );

    app.handle_server_event(crate::protocol::ServerEvent::MessageEnd, &mut remote);
    app.handle_server_event(crate::protocol::ServerEvent::Done { id: 0 }, &mut remote);

    assert!(
        !app.is_processing,
        "terminal Done must settle the adopted turn"
    );
    assert!(matches!(app.status, ProcessingStatus::Idle));
    assert!(app.processing_started.is_none());
    assert!(
        app.display_messages
            .iter()
            .any(|message| message.role == "assistant"
                && message.content.contains("Wake turn streaming text")),
        "adopted turn's streamed text should commit to the transcript"
    );
}

#[test]
fn test_externally_started_tool_turn_shows_running_tool_status() {
    let mut app = create_test_app();
    app.is_remote = true;
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    app.handle_server_event(
        crate::protocol::ServerEvent::ToolStart {
            id: "tool_1".to_string(),
            name: "bash".to_string(),
        },
        &mut remote,
    );

    assert!(app.is_processing);
    assert!(
        matches!(&app.status, ProcessingStatus::RunningTool(name) if name == "bash"),
        "adopted tool turn should show the running tool, got {:?}",
        app.status
    );
}

#[test]
fn test_remote_fork_with_prompt_stages_split_prompt() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.is_remote = true;
        app.input = "/fork explore plan b".to_string();
        app.cursor_pos = app.input.len();

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();

        rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
            .expect("/fork <prompt> should launch split request");

        assert!(app.pending_split_prompt.is_some());
        assert_eq!(app.pending_split_label.as_deref(), Some("Prompt"));
        assert!(!app.pending_split_request);

        app.handle_server_event(
            crate::protocol::ServerEvent::SplitResponse {
                id: 1,
                new_session_id: "session_fork_prompt_child".to_string(),
                new_session_name: "fork_prompt_child".to_string(),
            },
            &mut remote,
        );

        let restored = App::restore_input_for_reload("session_fork_prompt_child")
            .expect("forked session should stage the prompt");
        assert_eq!(restored.input, "explore plan b");
        assert!(restored.submit_on_restore);
        assert!(app.pending_split_prompt.is_none());
    });
}

#[test]
fn test_remote_btw_stages_question_in_forked_session() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.is_remote = true;
        app.input = "/btw what are we doing?".to_string();
        app.cursor_pos = app.input.len();

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();

        rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
            .expect("/btw should launch split request");

        assert!(app.pending_split_prompt.is_some());

        app.handle_server_event(
            crate::protocol::ServerEvent::SplitResponse {
                id: 1,
                new_session_id: "session_btw_child".to_string(),
                new_session_name: "btw_child".to_string(),
            },
            &mut remote,
        );

        let restored = App::restore_input_for_reload("session_btw_child")
            .expect("btw fork should stage the question");
        assert_eq!(restored.input, "what are we doing?");
        assert!(restored.submit_on_restore);
    });
}

#[test]
fn test_remote_fork_without_prompt_splits_immediately() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.input = "/fork".to_string();
    app.cursor_pos = app.input.len();

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
        .expect("/fork should send split request");

    assert!(app.pending_split_prompt.is_none());
    assert!(
        app.display_messages()
            .iter()
            .any(|msg| msg.content.contains("Forking session...")),
        "bare /fork should split immediately like /split"
    );
}

#[test]
fn test_credential_failure_breaker_trips_after_consecutive_auth_errors() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    // Auto-poke on with an auto-retryable pending message: this is the
    // runaway-loop shape that produced thousands of 401s per session.
    app.auto_poke_incomplete_todos = true;

    for attempt in 0..App::CREDENTIAL_FAILURE_BREAKER_THRESHOLD {
        app.rate_limit_pending_message = Some(PendingRemoteMessage {
            content: "poke".to_string(),
            images: vec![],
            is_system: true,
            system_reminder: None,
            auto_retry: true,
            retry_attempts: 0,
            retry_at: None,
        });
        app.is_processing = true;
        app.status = ProcessingStatus::Streaming;
        app.handle_server_event(
            crate::protocol::ServerEvent::Error {
                id: 100 + u64::from(attempt),
                message: "401 Unauthorized: invalid api key".to_string(),
                retry_after_secs: None,
            },
            &mut remote,
        );
    }

    assert!(
        app.rate_limit_pending_message.is_none(),
        "breaker must clear the pending auto-retry"
    );
    assert!(
        !app.auto_poke_incomplete_todos,
        "breaker must disable auto-poke"
    );
    assert!(app.overnight_auto_poke.is_none());
    assert_eq!(app.consecutive_credential_failures, 0);
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.role == "error" && m.content.contains("Stopped automatic retries")),
        "breaker must surface an actionable stop message"
    );
}

#[test]
fn test_credential_failure_breaker_streak_resets_on_other_errors() {
    let mut app = create_test_app();

    assert!(!app.note_error_for_credential_breaker("401 unauthorized"));
    assert!(!app.note_error_for_credential_breaker("invalid api key"));
    // A non-credential error resets the streak: mixed transient failures must
    // not trip the breaker.
    assert!(!app.note_error_for_credential_breaker("500 internal server error"));
    assert_eq!(app.consecutive_credential_failures, 0);
    assert!(!app.note_error_for_credential_breaker("401 unauthorized"));
    assert!(!app.note_error_for_credential_breaker("token expired"));
    assert!(app.note_error_for_credential_breaker("unauthorized"));
}

#[test]
fn test_credential_failure_breaker_resets_on_turn_success() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    assert!(!app.note_error_for_credential_breaker("401 unauthorized"));
    assert!(!app.note_error_for_credential_breaker("401 unauthorized"));

    app.is_processing = true;
    app.current_message_id = Some(7);
    app.handle_server_event(crate::protocol::ServerEvent::Done { id: 7 }, &mut remote);

    assert_eq!(
        app.consecutive_credential_failures, 0,
        "a successful turn must reset the credential-failure streak"
    );
}
