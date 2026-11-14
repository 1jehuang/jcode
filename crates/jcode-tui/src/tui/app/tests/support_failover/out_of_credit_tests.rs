use super::*;

fn remote_profile_failover_prompt() -> crate::provider::ProviderFailoverPrompt {
    crate::provider::ProviderFailoverPrompt {
        from_provider: "prof-a:glm-5.3".to_string(),
        from_label: "prof-a".to_string(),
        to_provider: "prof-b:glm-5.3".to_string(),
        to_label: "prof-b (glm-5.3)".to_string(),
        target_route: Some("prof-b:glm-5.3".to_string()),
        reason: "out of credit (HTTP 402: Insufficient credits)".to_string(),
        estimated_input_chars: 400,
        estimated_input_tokens: 100,
    }
}

fn remote_app_with_failed_turn() -> App {
    let mut app = create_test_app();
    app.is_remote = true;
    app.last_submitted_input = Some("say hi".to_string());
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "say hi".to_string(),
        images: vec![],
        is_system: false,
        system_reminder: None,
        auto_retry: false,
        retry_attempts: 0,
        retry_at: None,
        overload_attempts: 0,
    });
    app
}

#[test]
fn remote_out_of_credit_prompt_runs_countdown_then_switches_and_resends() {
    with_temp_jcode_home(|| {
        write_test_config("[provider]\ncross_provider_failover = \"countdown\"\n");
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        let mut app = remote_app_with_failed_turn();

        app.handle_server_event(
            crate::protocol::ServerEvent::Error {
                id: 1,
                message: failover_error_message(&remote_profile_failover_prompt()),
                retry_after_secs: None,
                server_resumes: false,
            },
            &mut remote,
        );
        let pending = app
            .pending_provider_failover
            .as_ref()
            .expect("remote session must arm the countdown");
        assert_eq!(pending.prompt.to_provider, "prof-b:glm-5.3");
        assert!(app.pending_model_switch.is_none(), "nothing sent before the deadline");
        assert!(app.pending_fallback_resend.is_none());
        assert!(
            app.rate_limit_pending_message.is_none(),
            "no blind retry to the out-of-credit profile"
        );

        if let Some(pending) = app.pending_provider_failover.as_mut() {
            pending.deadline = Instant::now() - Duration::from_secs(1);
        }
        app.maybe_progress_provider_failover_countdown();
        assert!(app.pending_provider_failover.is_none());
        assert_eq!(app.pending_model_switch.as_deref(), Some("prof-b:glm-5.3"));
        let resend = app
            .pending_fallback_resend
            .as_ref()
            .expect("failed turn staged for resend after the switch");
        assert_eq!(resend.content, "say hi");
    });
}

#[test]
fn remote_out_of_credit_countdown_esc_cancels_without_sending() {
    with_temp_jcode_home(|| {
        write_test_config("[provider]\ncross_provider_failover = \"countdown\"\n");
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        let mut app = remote_app_with_failed_turn();

        app.handle_server_event(
            crate::protocol::ServerEvent::Error {
                id: 1,
                message: failover_error_message(&remote_profile_failover_prompt()),
                retry_after_secs: None,
                server_resumes: false,
            },
            &mut remote,
        );
        assert!(app.pending_provider_failover.is_some());
        rt.block_on(app.handle_remote_key(KeyCode::Esc, KeyModifiers::NONE, &mut remote))
            .expect("esc");
        assert!(app.pending_provider_failover.is_none());
        assert!(app.pending_model_switch.is_none());
        assert!(app.pending_fallback_resend.is_none());
        assert_eq!(app.input, "say hi", "the prompt goes back to the input box");
        let last = app.display_messages.last().expect("message");
        assert!(last.content.contains("Auto-switch canceled"), "{}", last.content);

        // The deadline passing later must not switch anything.
        app.maybe_progress_provider_failover_countdown();
        assert!(app.pending_model_switch.is_none());
    });
}

#[test]
fn remote_out_of_credit_prompt_in_manual_mode_only_explains() {
    with_temp_jcode_home(|| {
        write_test_config("[provider]\ncross_provider_failover = \"manual\"\n");
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        let mut app = remote_app_with_failed_turn();
        app.handle_server_event(
            crate::protocol::ServerEvent::Error {
                id: 1,
                message: failover_error_message(&remote_profile_failover_prompt()),
                retry_after_secs: None,
                server_resumes: false,
            },
            &mut remote,
        );
        assert!(app.pending_provider_failover.is_none());
        assert!(app.pending_model_switch.is_none());
        assert!(app.pending_fallback_resend.is_none());
        let last = app.display_messages.last().expect("message");
        assert!(last.content.contains("Use /model to switch"), "{}", last.content);
    });
}

#[test]
fn test_local_failover_revalidates_runtime_source_pool_at_expiry() {
    for (initial_provider, source_key, exclude) in [
        ("claude", "anthropic-api", false),
        ("openai", "openai:openai-otter", true),
    ] {
        with_temp_jcode_home(|| {
            write_test_config(
                "[provider]\ncross_provider_failover = \"countdown\"\nexhausted_usage_immediate_failover = false\n",
            );
            let (mut app, active_provider) = create_switchable_test_app(initial_provider);
            // Deliberately stale metadata must not override runtime identity.
            app.session.route_api_method = Some("claude-oauth@stale-account".into());
            let prompt = crate::provider::ProviderFailoverPrompt {
                from_provider: "stale-provider".into(),
                from_label: "Previous account".into(),
                to_provider: "openai".into(),
                to_label: "OpenAI fox".into(),
                target_route: Some("openai-oauth@openai-fox:gpt-5.4".into()),
                reason: "OAuth usage exhausted".into(),
                estimated_input_chars: 400,
                estimated_input_tokens: 100,
            };
            app.handle_turn_error(failover_error_message(&prompt));
            assert!(app.pending_provider_failover.is_some());
            let mut pool = crate::auth::account_pool::AccountPool::load();
            pool.members.retain(|member| member != source_key);
            if exclude {
                pool.excluded.push(source_key.into());
            }
            pool.save().unwrap();
            app.pending_provider_failover.as_mut().unwrap().deadline =
                Instant::now() - Duration::from_secs(1);
            app.maybe_progress_provider_failover_countdown();
            assert_eq!(active_provider.lock().unwrap().as_str(), initial_provider);
            assert!(!app.pending_turn);
            assert!(app.pending_provider_failover.is_none());
            assert!(app.display_messages.iter().any(|message| {
                message
                    .content
                    .contains("Source is no longer in the auto-switch pool")
            }));
        });
    }
}
