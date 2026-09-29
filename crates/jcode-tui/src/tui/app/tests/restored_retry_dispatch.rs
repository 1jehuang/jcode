#[test]
fn test_restored_retry_identical_fresh_prompt_is_independent() {
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let id = "identical_fresh_retry";
        let mut app = create_test_app();
        app.restored_retries.push(PendingRemoteMessage {
            content: "same prompt".into(),
            images: vec![("image/png".into(), "payload".into())],
            is_system: false,
            system_reminder: None,
            auto_retry: true,
            retry_attempts: 2,
            retry_at: None,
        });
        app.save_input_for_reload(id);
        app.passive_restart_restore = true;
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.set_session_id(id.into());
        remote.mark_history_loaded();
        let fresh_id = rt
            .block_on(super::remote::begin_remote_send(
                &mut app,
                &mut remote,
                "same prompt".into(),
                vec![("image/png".into(), "payload".into())],
                false,
                None,
                false,
                0,
            ))
            .unwrap();
        assert!(
            app.restored_retry_delivery.is_none(),
            "fresh prompt must not own saved retry"
        );
        app.save_input_for_reload(id);
        let saved = App::restore_input_for_reload(id).unwrap();
        assert_eq!(saved.restored_retries.len(), 1);
        assert_eq!(
            saved.input, "same prompt",
            "fresh in-flight input must survive a save"
        );
        app.handle_server_event(
            crate::protocol::ServerEvent::Done { id: fresh_id },
            &mut remote,
        );
        assert_eq!(
            App::new_for_remote(Some(id.into())).restored_retries.len(),
            1
        );
        rt.block_on(super::remote::process_remote_followups(
            &mut app,
            &mut remote,
        ));
        let retry_id = app.current_message_id.unwrap();
        assert_ne!(fresh_id, retry_id);
        assert_eq!(
            app.restored_retry_delivery.as_ref().unwrap().request_id,
            retry_id
        );
        app.handle_server_event(
            crate::protocol::ServerEvent::Done { id: retry_id },
            &mut remote,
        );
        assert!(
            App::new_for_remote(Some(id.into()))
                .restored_retries
                .is_empty()
        );
    });
}

#[test]
fn test_restored_retry_busy_rejection_can_redispatch_without_duplicates() {
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let id = "busy_saved_retry";
        let mut app = create_test_app();
        app.restored_retries.push(PendingRemoteMessage {
            content: "saved retry".into(),
            images: vec![],
            is_system: true,
            system_reminder: Some("saved reminder".into()),
            auto_retry: true,
            retry_attempts: 0,
            retry_at: None,
        });
        app.save_input_for_reload(id);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.set_session_id(id.into());
        remote.mark_history_loaded();
        rt.block_on(super::remote::process_remote_followups(
            &mut app,
            &mut remote,
        ));
        let first_id = app.current_message_id.unwrap();
        app.handle_server_event(
            crate::protocol::ServerEvent::Error {
                id: first_id,
                message: "Already processing a message".into(),
                retry_after_secs: None,
            },
            &mut remote,
        );
        assert!(
            app.restored_retry_delivery.is_none(),
            "rejection must release delivery"
        );
        assert!(
            app.queued_messages.is_empty(),
            "saved retry must not also become a follow-up"
        );
        assert!(app.hidden_queued_system_messages.is_empty());
        app.handle_server_event(
            crate::protocol::ServerEvent::Done { id: first_id },
            &mut remote,
        );
        assert_eq!(app.restored_retries.len(), 1);
        rt.block_on(super::remote::process_remote_followups(
            &mut app,
            &mut remote,
        ));
        let second_id = app.current_message_id.unwrap();
        assert_ne!(first_id, second_id);
        app.handle_server_event(
            crate::protocol::ServerEvent::Done { id: first_id },
            &mut remote,
        );
        assert_eq!(app.restored_retries.len(), 1);
        app.handle_server_event(
            crate::protocol::ServerEvent::Done { id: second_id },
            &mut remote,
        );
        assert!(
            App::new_for_remote(Some(id.into()))
                .restored_retries
                .is_empty()
        );
    });
}

#[test]
fn test_restored_retry_rate_limit_keeps_provenance_and_deadline() {
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let id = "rate_limited_saved_retry";
        let mut app = create_test_app();
        app.restored_retries.push(PendingRemoteMessage {
            content: "saved retry".into(),
            images: vec![],
            is_system: false,
            system_reminder: None,
            auto_retry: true,
            retry_attempts: 2,
            retry_at: None,
        });
        app.save_input_for_reload(id);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.set_session_id(id.into());
        remote.mark_history_loaded();
        rt.block_on(super::remote::process_remote_followups(
            &mut app,
            &mut remote,
        ));
        let first_id = app.current_message_id.unwrap();
        app.handle_server_event(
            crate::protocol::ServerEvent::Error {
                id: first_id,
                message: "rate limited".into(),
                retry_after_secs: Some(60),
            },
            &mut remote,
        );
        assert!(app.restored_retry_delivery.is_none());
        app.save_input_for_reload(id);
        let saved = App::restore_input_for_reload(id).unwrap();
        assert_eq!(saved.restored_retries.len(), 1);
        assert!(saved.restored_retries[0].retry_at.unwrap() > Instant::now());
        assert!(saved.rate_limit_pending_message.is_none());
        rt.block_on(super::remote::process_remote_followups(
            &mut app,
            &mut remote,
        ));
        assert!(app.current_message_id.is_none());
        app.rate_limit_reset = Some(Instant::now());
        rt.block_on(super::remote::handle_tick(&mut app, &mut remote));
        let second_id = app.current_message_id.unwrap();
        assert_ne!(first_id, second_id);
        assert_eq!(
            app.restored_retry_delivery.as_ref().unwrap().request_id,
            second_id
        );
        app.handle_server_event(
            crate::protocol::ServerEvent::Done { id: first_id },
            &mut remote,
        );
        assert_eq!(app.restored_retries.len(), 1);
        app.handle_server_event(
            crate::protocol::ServerEvent::Done { id: second_id },
            &mut remote,
        );
        assert!(
            App::new_for_remote(Some(id.into()))
                .restored_retries
                .is_empty()
        );
    });
}

#[test]
fn test_restored_retry_terminal_failure_waits_for_fresh_submission() {
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut app = create_test_app();
        app.restored_retries.push(PendingRemoteMessage {
            content: "saved retry".into(),
            images: vec![],
            is_system: false,
            system_reminder: None,
            auto_retry: true,
            retry_attempts: u8::MAX,
            retry_at: None,
        });
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.set_session_id("terminal_saved_retry".into());
        remote.mark_history_loaded();
        rt.block_on(super::remote::process_remote_followups(
            &mut app,
            &mut remote,
        ));
        let first_id = app.current_message_id.unwrap();
        app.handle_server_event(
            crate::protocol::ServerEvent::Error {
                id: first_id,
                message: "Request failed".into(),
                retry_after_secs: None,
            },
            &mut remote,
        );
        app.handle_server_event(
            crate::protocol::ServerEvent::Done { id: first_id },
            &mut remote,
        );
        rt.block_on(super::remote::process_remote_followups(
            &mut app,
            &mut remote,
        ));
        assert!(
            !app.is_processing,
            "saved retries must not bypass the retry limit"
        );
        assert_eq!(app.restored_retries.len(), 1);
        let fresh_id = rt
            .block_on(super::remote::begin_remote_send(
                &mut app,
                &mut remote,
                "fresh prompt".into(),
                vec![],
                false,
                None,
                false,
                0,
            ))
            .unwrap();
        app.handle_server_event(
            crate::protocol::ServerEvent::Done { id: fresh_id },
            &mut remote,
        );
        rt.block_on(super::remote::process_remote_followups(
            &mut app,
            &mut remote,
        ));
        assert!(app.restored_retry_delivery.is_some());
    });
}

#[test]
fn test_restored_retry_disconnect_releases_delivery_without_duplicate() {
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut app = create_test_app();
        app.restored_retries.push(PendingRemoteMessage {
            content: "saved follow-up".into(),
            images: vec![],
            is_system: true,
            system_reminder: None,
            auto_retry: false,
            retry_attempts: 0,
            retry_at: None,
        });
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.set_session_id("disconnected_saved_retry".into());
        remote.mark_history_loaded();
        rt.block_on(super::remote::process_remote_followups(
            &mut app,
            &mut remote,
        ));
        let mut state = super::remote::RemoteRunState::default();
        super::remote::handle_disconnect(&mut app, &mut state, None);
        assert!(app.restored_retry_delivery.is_none());
        assert!(app.queued_messages.is_empty());
        assert_eq!(app.restored_retries.len(), 1);
        app.is_processing = false;
        rt.block_on(super::remote::process_remote_followups(
            &mut app,
            &mut remote,
        ));
        assert!(app.restored_retry_delivery.is_some());
    });
}

#[test]
fn test_stopped_restored_retry_stays_stopped_after_reopen() {
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let id = "stopped_saved_retry";
        let mut app = create_test_app();
        app.remote_session_id = Some(id.into());
        app.restored_retries.push(PendingRemoteMessage {
            content: "saved retry".into(),
            images: vec![],
            is_system: false,
            system_reminder: None,
            auto_retry: true,
            retry_attempts: u8::MAX,
            retry_at: None,
        });
        app.save_input_for_reload(id);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.set_session_id(id.into());
        remote.mark_history_loaded();
        rt.block_on(super::remote::process_remote_followups(
            &mut app,
            &mut remote,
        ));
        let failed_id = app.current_message_id.unwrap();
        app.handle_server_event(
            crate::protocol::ServerEvent::Error {
                id: failed_id,
                message: "Request failed".into(),
                retry_after_secs: None,
            },
            &mut remote,
        );
        app.handle_server_event(
            crate::protocol::ServerEvent::Done { id: failed_id },
            &mut remote,
        );
        assert_eq!(app.restored_retries.len(), 1);
        let checkpoint: serde_json::Value = serde_json::from_slice(
            &std::fs::read(
                crate::storage::jcode_dir()
                    .unwrap()
                    .join(format!("client-input-{id}")),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(checkpoint["restored_retry_stopped"], true);
        drop(app);

        let mut reopened = App::new_for_remote(Some(id.into()));
        assert_eq!(reopened.restored_retries.len(), 1);
        rt.block_on(super::remote::process_remote_followups(
            &mut reopened,
            &mut remote,
        ));
        assert!(
            reopened.current_message_id.is_none(),
            "reopening must not start a stopped retry"
        );
        reopened.save_input_for_reload(id);
        drop(reopened);
        let mut reopened = App::new_for_remote(Some(id.into()));
        rt.block_on(super::remote::process_remote_followups(
            &mut reopened,
            &mut remote,
        ));
        assert!(
            reopened.current_message_id.is_none(),
            "saving and reopening again must keep the retry stopped"
        );
        let fresh_id = rt
            .block_on(super::remote::begin_remote_send(
                &mut reopened,
                &mut remote,
                "fresh prompt".into(),
                vec![],
                false,
                None,
                false,
                0,
            ))
            .unwrap();
        reopened.handle_server_event(
            crate::protocol::ServerEvent::Done { id: fresh_id },
            &mut remote,
        );
        rt.block_on(super::remote::process_remote_followups(
            &mut reopened,
            &mut remote,
        ));
        assert!(reopened.restored_retry_delivery.is_some());
    });
}

#[test]
fn test_failover_prompt_stops_restored_retry_until_fresh_submission() {
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let id = "failover_saved_retry";
        let mut app = create_test_app();
        app.remote_session_id = Some(id.into());
        app.restored_retries.push(PendingRemoteMessage {
            content: "saved retry".into(),
            images: vec![],
            is_system: false,
            system_reminder: None,
            auto_retry: true,
            retry_attempts: 0,
            retry_at: None,
        });
        app.save_input_for_reload(id);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.set_session_id(id.into());
        remote.mark_history_loaded();
        rt.block_on(super::remote::process_remote_followups(
            &mut app,
            &mut remote,
        ));
        let request_id = app.current_message_id.unwrap();
        let prompt = crate::provider::ProviderFailoverPrompt {
            from_provider: "openai".into(),
            from_label: "OpenAI".into(),
            to_provider: "anthropic".into(),
            to_label: "Anthropic".into(),
            reason: "unavailable".into(),
            estimated_input_chars: 11,
            estimated_input_tokens: 3,
        };
        app.handle_server_event(
            crate::protocol::ServerEvent::Error {
                id: request_id,
                message: prompt.to_error_message(),
                retry_after_secs: None,
            },
            &mut remote,
        );
        assert!(app.restored_retry_stopped);
        drop(app);
        let mut reopened = App::new_for_remote(Some(id.into()));
        rt.block_on(super::remote::process_remote_followups(
            &mut reopened,
            &mut remote,
        ));
        assert!(reopened.current_message_id.is_none());
        assert_eq!(reopened.restored_retries.len(), 1);
    });
}
