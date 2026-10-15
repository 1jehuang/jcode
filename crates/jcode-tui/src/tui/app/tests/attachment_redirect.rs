use super::*;

fn local_image_redirect_app() -> App {
    let mut app = create_test_app();
    app.is_processing = true;
    app.interleave_message = Some("describe this image".to_string());
    app.interleave_images = vec![("image/png".to_string(), "aGk=".to_string())];
    app.handle_key(KeyCode::Esc, KeyModifiers::empty()).unwrap();
    app
}

#[test]
fn test_local_escape_redirect_queued_session_payload_contains_image_and_text() {
    let mut app = local_image_redirect_app();
    app.prepare_queued_local_turn();
    assert!(
        matches!(app.session.messages.last().unwrap().content.as_slice(),
        [ContentBlock::Image { media_type, data }, ContentBlock::Text { text, .. }]
        if media_type == "image/png" && data == "aGk=" && text == "describe this image")
    );
}

#[test]
fn test_local_escape_redirect_queued_provider_payload_contains_image_and_text() {
    let mut app = local_image_redirect_app();
    app.prepare_queued_local_turn();
    assert!(
        matches!(app.materialized_provider_messages().last().unwrap().content.as_slice(),
        [ContentBlock::Image { media_type, data }, ContentBlock::Text { text, .. }]
        if media_type == "image/png" && data == "aGk=" && text == "describe this image")
    );
}

#[test]
fn test_local_escape_redirect_consumes_only_queued_images() {
    let mut app = local_image_redirect_app();
    app.pending_images
        .push(("image/jpeg".to_string(), "draft".to_string()));
    app.prepare_queued_local_turn();
    assert_eq!(
        (app.queued_message_images, app.pending_images),
        (
            vec![],
            vec![("image/jpeg".to_string(), "draft".to_string())]
        )
    );
}

#[test]
fn test_retrieving_redirect_for_edit_restores_text_and_images_together() {
    let mut app = local_image_redirect_app();
    super::input::retrieve_pending_message_for_edit(&mut app);
    assert_eq!(
        (app.input, app.pending_images, app.queued_message_images),
        (
            "describe this image [image 1]".to_string(),
            vec![("image/png".to_string(), "aGk=".to_string())],
            vec![]
        )
    );
}

#[test]
fn test_discarded_user_queue_cannot_leak_images_to_next_payload() {
    let mut app = local_image_redirect_app();
    app.queued_messages.clear();
    app.hidden_queued_system_messages
        .push("internal reminder".to_string());
    app.prepare_queued_local_turn();
    app.queued_messages.push("unrelated prompt".to_string());
    app.prepare_queued_local_turn();
    assert!(
        matches!(app.session.messages.last().unwrap().content.as_slice(),
        [ContentBlock::Text { text, .. }] if text == "unrelated prompt")
    );
}

#[test]
fn test_session_recovery_discards_redirect_images() {
    let mut app = local_image_redirect_app();
    app.recover_session_without_tools();
    app.queued_messages.push("new session prompt".to_string());
    app.prepare_queued_local_turn();
    assert!(
        matches!(app.session.messages.last().unwrap().content.as_slice(),
        [ContentBlock::Text { text, .. }] if text == "new session prompt")
    );
}

#[test]
fn test_remote_session_switch_discards_redirect_images() {
    let mut app = local_image_redirect_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    app.is_remote = true;
    app.remote_session_id = Some("old_session".to_string());
    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "new_session".to_string(),
            messages: vec![],
            images: vec![],
            provider_name: Some("mock".to_string()),
            provider_model: Some("mock-model".to_string()),
            provider_oauth_email: None,
            subagent_model: None,
            agent_model_overrides: Default::default(),
            autoreview_enabled: None,
            autojudge_enabled: None,
            available_models: vec![],
            available_model_routes: vec![],
            mcp_servers: vec![],
            skills: vec![],
            total_tokens: None,
            token_usage_totals: None,
            all_sessions: vec![],
            client_count: None,
            is_canary: None,
            reload_recovery: None,
            server_version: None,
            server_name: None,
            server_icon: None,
            server_has_update: None,
            was_interrupted: None,
            connection_type: None,
            status_detail: None,
            upstream_provider: None,
            resolved_credential: None,
            reasoning_effort: None,
            service_tier: None,
            account_labels: vec![],
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: None,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
            applets: Default::default(),
        },
        &mut remote,
    );
    assert_eq!(
        (app.queued_messages, app.queued_message_images),
        (vec![], vec![])
    );
}

#[test]
fn test_remote_clear_discards_redirect_images() {
    let mut app = local_image_redirect_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    app.is_processing = false;
    app.input = "/clear".to_string();
    app.cursor_pos = app.input.len();
    rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
        .unwrap();
    assert_eq!(
        (app.queued_messages, app.queued_message_images),
        (vec![], vec![])
    );
}

fn reload_queued_image_app(app: &App, name: &str) -> App {
    let session_id = format!("test-queued-image-{name}-{}", std::process::id());
    app.save_input_for_reload(&session_id);
    let restored = App::restore_input_for_reload(&session_id).unwrap();
    let mut reloaded = create_test_app();
    reloaded.apply_restored_reload_input(restored);
    reloaded
}

#[test]
fn test_redirect_image_reload_reaches_queued_session_payload() {
    let app = local_image_redirect_app();
    let mut reloaded = reload_queued_image_app(&app, "redirect");
    reloaded.prepare_queued_local_turn();
    assert!(
        matches!(reloaded.session.messages.last().unwrap().content.as_slice(),
        [ContentBlock::Image { media_type, data }, ContentBlock::Text { text, .. }]
        if media_type == "image/png" && data == "aGk=" && text == "describe this image")
    );
}

#[test]
fn test_interleave_image_reload_reaches_queued_provider_payload() {
    let mut app = create_test_app();
    app.interleave_message = Some("describe this image".to_string());
    app.interleave_images = vec![("image/png".to_string(), "aGk=".to_string())];
    let mut reloaded = reload_queued_image_app(&app, "interleave");
    reloaded.prepare_queued_local_turn();
    assert!(
        matches!(reloaded.materialized_provider_messages().last().unwrap().content.as_slice(),
        [ContentBlock::Image { media_type, data }, ContentBlock::Text { text, .. }]
        if media_type == "image/png" && data == "aGk=" && text == "describe this image")
    );
}

#[test]
fn test_inflight_queued_image_reload_preserves_existing_queued_images() {
    let mut app = local_image_redirect_app();
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "inflight image prompt".to_string(),
        images: vec![("image/jpeg".to_string(), "inflight".to_string())],
        is_system: true,
        system_reminder: None,
        auto_retry: false,
        retry_attempts: 0,
        retry_at: None,
        overload_attempts: 0,
    });
    let reloaded = reload_queued_image_app(&app, "inflight");
    assert_eq!(
        (reloaded.queued_messages, reloaded.queued_message_images),
        (
            vec![
                "inflight image prompt".to_string(),
                "describe this image".to_string()
            ],
            vec![
                ("image/jpeg".to_string(), "inflight".to_string()),
                ("image/png".to_string(), "aGk=".to_string())
            ]
        )
    );
}

#[test]
fn test_disconnect_recovers_inflight_image_with_queued_text() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "inflight image prompt".to_string(),
        images: vec![("image/png".to_string(), "aGk=".to_string())],
        is_system: true,
        system_reminder: None,
        auto_retry: false,
        retry_attempts: 0,
        retry_at: None,
        overload_attempts: 0,
    });
    let mut state = remote::RemoteRunState::default();
    remote::handle_disconnect(&mut app, &mut state, None);
    assert_eq!(
        (app.queued_messages, app.queued_message_images),
        (
            vec!["inflight image prompt".to_string()],
            vec![("image/png".to_string(), "aGk=".to_string())]
        )
    );
}

#[test]
fn test_disconnect_recovers_interleave_image_without_touching_composer() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    app.interleave_message = Some("describe this image".to_string());
    app.interleave_images = vec![("image/png".to_string(), "aGk=".to_string())];
    let mut state = remote::RemoteRunState::default();
    remote::handle_disconnect(&mut app, &mut state, None);
    assert_eq!(
        (
            app.queued_messages,
            app.queued_message_images,
            app.pending_images
        ),
        (
            vec!["describe this image".to_string()],
            vec![("image/png".to_string(), "aGk=".to_string())],
            vec![]
        )
    );
}

#[test]
fn test_reload_empty_queue_discards_orphan_images() {
    with_temp_jcode_home(|| {
        let mut app = local_image_redirect_app();
        let session_id = format!("test-queued-image-orphan-{}", std::process::id());
        app.save_input_for_reload(&session_id);
        let mut restored = App::restore_input_for_reload(&session_id).unwrap();
        restored.queued_messages.clear();
        app.apply_restored_reload_input(restored);
        assert!(app.queued_message_images.is_empty());
    });
}

#[test]
fn test_reload_orphan_images_do_not_attach_to_recovered_soft_interrupt() {
    with_temp_jcode_home(|| {
        let app = local_image_redirect_app();
        let session_id = format!("test-queued-image-unrelated-{}", std::process::id());
        app.save_input_for_reload(&session_id);
        let path = crate::storage::jcode_dir()
            .unwrap()
            .join(format!("client-input-{session_id}"));
        let mut data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        data["queued_messages"] = serde_json::json!([]);
        data["pending_soft_interrupt_resend"] = serde_json::json!(["unrelated recovered prompt"]);
        std::fs::write(path, data.to_string()).unwrap();
        let restored = App::restore_input_for_reload(&session_id).unwrap();
        let mut reloaded = create_test_app();
        reloaded.apply_restored_reload_input(restored);
        reloaded.prepare_queued_local_turn();
        assert!(
            matches!(reloaded.session.messages.last().unwrap().content.as_slice(),
            [ContentBlock::Text { text, .. }] if text == "unrelated recovered prompt")
        );
    });
}

#[test]
fn test_reload_legacy_queue_without_images_restores_text() {
    with_temp_jcode_home(|| {
        let app = local_image_redirect_app();
        let session_id = format!("test-queued-image-legacy-{}", std::process::id());
        app.save_input_for_reload(&session_id);
        let path = crate::storage::jcode_dir()
            .unwrap()
            .join(format!("client-input-{session_id}"));
        let mut data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        data.as_object_mut()
            .unwrap()
            .remove("queued_message_images");
        data.as_object_mut().unwrap().remove("interleave_images");
        std::fs::write(path, data.to_string()).unwrap();
        let restored = App::restore_input_for_reload(&session_id).unwrap();
        let mut reloaded = create_test_app();
        reloaded.apply_restored_reload_input(restored);
        assert_eq!(
            (reloaded.queued_messages, reloaded.queued_message_images),
            (vec!["describe this image".to_string()], vec![])
        );
    });
}
