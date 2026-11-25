fn queued_recovery_image() -> (String, String) {
    ("image/png".into(), "aGk=".into())
}

fn reload_image_followup(app: &App) -> App {
    app.save_input_for_reload("image-followup-recovery");
    let saved = App::restore_input_for_reload("image-followup-recovery").unwrap();
    let mut restored = create_test_app();
    restored.apply_restored_reload_input(saved);
    restored
}

#[test]
fn image_followup_recovery_reload_keeps_interleave_images() {
    with_temp_jcode_home(|| {
        for text in ["", "  ", "describe this"] {
            let mut app = create_test_app();
            app.interleave_message = Some(text.into());
            app.interleave_images = vec![queued_recovery_image()];
            let mut restored = reload_image_followup(&app);
            assert_eq!(restored.queued_messages, vec![text]);
            assert_eq!(restored.queued_images, vec![queued_recovery_image()]);
            assert!(super::input::retrieve_pending_message_for_edit(
                &mut restored
            ));
            let prepared = super::input::take_prepared_input(&mut restored);
            assert_eq!(prepared.expanded, text);
            assert_eq!(prepared.images, vec![queued_recovery_image()]);
        }
    });
}

#[test]
fn image_followup_recovery_reload_requeues_inflight_image_only() {
    with_temp_jcode_home(|| {
        for text in ["", "  "] {
            let mut app = create_test_app();
            app.rate_limit_pending_message = Some(PendingRemoteMessage {
                jarvis_voice: false,
                content: text.into(),
                images: vec![queued_recovery_image()],
                is_system: true,
                system_reminder: None,
                auto_retry: false,
                retry_attempts: 0,
                retry_at: None,
            });
            let restored = reload_image_followup(&app);
            assert!(restored.rate_limit_pending_message.is_none());
            assert!(restored.rate_limit_reset.is_none());
            assert_eq!(restored.queued_messages, vec![text]);
            assert_eq!(restored.queued_images, vec![queued_recovery_image()]);
        }
    });
}

#[test]
fn image_followup_recovery_disconnect_keeps_inflight_image_only() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut app = create_test_app();
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        jarvis_voice: false,
        content: String::new(),
        images: vec![queued_recovery_image()],
        is_system: true,
        system_reminder: None,
        auto_retry: false,
        retry_attempts: 0,
        retry_at: None,
    });
    let mut state = remote::RemoteRunState::default();
    remote::handle_disconnect(&mut app, &mut state, None);
    assert_eq!(app.queued_messages, vec![String::new()]);
    assert_eq!(app.queued_images, vec![queued_recovery_image()]);
    assert!(app.pending_images.is_empty());
}

#[test]
fn image_followup_recovery_busy_rejection_keeps_image_only_prompt() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut app = create_test_app();
    app.is_remote = true;
    app.auto_poke_incomplete_todos = false;
    let mut connection = crate::tui::backend::RemoteConnection::dummy();
    connection.mark_history_loaded();
    rt.block_on(remote::begin_remote_send(
        &mut app,
        &mut connection,
        String::new(),
        vec![queued_recovery_image()],
        false,
        None,
        false,
        0,
    ))
    .unwrap();
    let id = app.current_message_id.unwrap();
    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id,
            message: "Already processing a message".into(),
            retry_after_secs: None,
        },
        &mut connection,
    );
    assert!(app.rate_limit_pending_message.is_none());
    assert_eq!(app.queued_messages, vec![String::new()]);
    assert_eq!(app.queued_images, vec![queued_recovery_image()]);
}

#[test]
fn image_followup_recovery_local_escape_keeps_images_out_of_new_composer() {
    let mut app = create_test_app();
    app.is_processing = true;
    app.interleave_message = Some("describe this".into());
    app.interleave_images = vec![queued_recovery_image()];
    app.handle_key(KeyCode::Esc, KeyModifiers::NONE).unwrap();
    assert_eq!(app.queued_messages, vec!["describe this"]);
    assert_eq!(app.queued_images, vec![queued_recovery_image()]);
    assert!(app.pending_images.is_empty());
}

fn assert_failed_queued_image_write_is_recoverable(tick: bool) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut app = create_test_app();
    app.is_remote = true;
    app.runtime_mode = super::AppRuntimeMode::RemoteClient;
    app.input = "newer draft".into();
    app.queued_messages.push(String::new());
    app.queued_images.push(queued_recovery_image());
    let mut connection = crate::tui::backend::RemoteConnection::dummy();
    connection.mark_history_loaded();
    drop(connection.take_dummy_peer());
    if tick {
        rt.block_on(remote::handle_tick(&mut app, &mut connection));
    } else {
        rt.block_on(remote::process_remote_followups(&mut app, &mut connection));
    }
    assert_eq!(app.queued_messages, vec![String::new()]);
    assert_eq!(app.queued_images, vec![queued_recovery_image()]);
    assert_eq!(app.input, "newer draft");
    assert!(app.pending_images.is_empty());
}

#[test]
fn image_followup_recovery_failed_followup_write_keeps_image_only() {
    assert_failed_queued_image_write_is_recoverable(false);
}

#[test]
fn image_followup_recovery_failed_tick_write_keeps_image_only() {
    assert_failed_queued_image_write_is_recoverable(true);
}

#[test]
fn image_followup_recovery_local_provider_payload_keeps_new_draft_separate() {
    for text in ["", "  ", "describe this"] {
        let mut app = create_test_app();
        app.queued_messages.push(text.into());
        app.queued_images.push(queued_recovery_image());
        let newer = ("image/jpeg".to_string(), "newer draft image".to_string());
        app.pending_images.push(newer.clone());
        app.input = "newer draft".into();
        app.prepare_queued_local_turn();
        let content = &app.session.messages.last().unwrap().content;
        assert!(content.iter().any(
            |block| matches!(block, ContentBlock::Image {media_type,data}
            if media_type=="image/png" && data=="aGk=")
        ));
        let provider = app.materialized_provider_messages();
        assert!(
            provider
                .last()
                .unwrap()
                .content
                .iter()
                .any(|block| matches!(block,
            ContentBlock::Image {media_type,data} if media_type=="image/png" && data=="aGk="))
        );
        assert!(
            provider
                .last()
                .unwrap()
                .content
                .iter()
                .all(|block| !matches!(block,
            ContentBlock::Image {media_type,..} if media_type=="image/jpeg"))
        );
        assert_eq!(app.pending_images, vec![newer]);
        assert!(app.queued_images.is_empty());
        assert_eq!(app.input, "newer draft");
    }
}

#[test]
fn image_followup_recovery_disk_orphans_do_not_join_recovered_text() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.track_pending_soft_interrupt(42, "recovered prompt".into());
        app.save_input_for_reload("image-followup-orphans");
        let path = crate::storage::jcode_dir()
            .unwrap()
            .join("client-input-image-followup-orphans");
        let mut data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        data["queued_images"] = serde_json::json!([["image/png", "orphaned queue"]]);
        data["interleave_images"] = serde_json::json!([["image/png", "orphaned interleave"]]);
        std::fs::write(&path, data.to_string()).unwrap();
        let saved = App::restore_input_for_reload("image-followup-orphans").unwrap();
        let mut restored = create_test_app();
        restored.apply_restored_reload_input(saved);
        restored.prepare_queued_local_turn();
        assert!(
            matches!(restored.session.messages.last().unwrap().content.as_slice(),
            [ContentBlock::Text {text,..}] if text=="recovered prompt")
        );
        assert!(restored.queued_images.is_empty());
    });
}

#[test]
fn image_followup_recovery_idle_interleave_sends_or_restores_complete_payload() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    for failed_write in [false, true] {
        for text in ["", "describe this"] {
            let mut app = create_test_app();
            app.is_remote = true;
            app.runtime_mode = super::AppRuntimeMode::RemoteClient;
            app.input = "newer draft".into();
            app.interleave_message = Some(text.into());
            app.interleave_images = vec![queued_recovery_image()];
            let mut connection = crate::tui::backend::RemoteConnection::dummy();
            connection.mark_history_loaded();
            if failed_write {
                drop(connection.take_dummy_peer());
            }
            rt.block_on(remote::process_remote_followups(&mut app, &mut connection));
            if failed_write {
                assert_eq!(app.queued_messages, vec![text]);
                assert_eq!(app.queued_images, vec![queued_recovery_image()]);
            } else {
                let sent = app.rate_limit_pending_message.as_ref().unwrap();
                assert_eq!(sent.content, text);
                assert_eq!(sent.images, vec![queued_recovery_image()]);
            }
            assert_eq!(app.input, "newer draft");
            assert!(app.pending_images.is_empty());
        }
    }
}

#[test]
fn image_followup_recovery_snapshot_keeps_payload_order_and_drops_queue_orphans() {
    with_temp_jcode_home(|| {
        for has_queue in [false, true] {
            let mut app = create_test_app();
            let queued = ("image/jpeg".into(), "queued or orphan".into());
            app.queued_images.push(queued.clone());
            if has_queue {
                app.queued_messages.push("later".into());
            }
            app.rate_limit_pending_message = Some(PendingRemoteMessage {
                jarvis_voice: false,
                content: String::new(),
                images: vec![queued_recovery_image()],
                is_system: true,
                system_reminder: None,
                auto_retry: false,
                retry_attempts: 0,
                retry_at: None,
            });
            let restored = reload_image_followup(&app);
            assert_eq!(
                restored.queued_messages,
                if has_queue {
                    vec!["", "later"]
                } else {
                    vec![""]
                }
            );
            assert_eq!(
                restored.queued_images,
                if has_queue {
                    vec![queued_recovery_image(), queued]
                } else {
                    vec![queued_recovery_image()]
                }
            );
        }
    });
}

#[test]
fn image_followup_recovery_local_discarded_queue_cannot_leak_images() {
    let mut app = create_test_app();
    app.queued_images.push(queued_recovery_image());
    app.hidden_queued_system_messages
        .push("internal reminder".into());
    app.prepare_queued_local_turn();
    assert!(app.queued_images.is_empty());
    app.queued_messages.push("unrelated prompt".into());
    app.prepare_queued_local_turn();
    assert!(
        matches!(app.session.messages.last().unwrap().content.as_slice(),
        [ContentBlock::Text {text,..}] if text=="unrelated prompt")
    );
}

#[test]
fn image_followup_recovery_legacy_and_malformed_image_fields_keep_text() {
    with_temp_jcode_home(|| {
        for malformed in [false, true] {
            let mut app = create_test_app();
            app.queued_messages.push("queued prompt".into());
            app.interleave_message = Some("staged prompt".into());
            app.save_input_for_reload("image-followup-legacy");
            let path = crate::storage::jcode_dir()
                .unwrap()
                .join("client-input-image-followup-legacy");
            let mut data: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            for key in ["queued_images", "interleave_images"] {
                if malformed {
                    data[key] = serde_json::json!(["invalid tuple"]);
                } else {
                    data.as_object_mut().unwrap().remove(key);
                }
            }
            std::fs::write(&path, data.to_string()).unwrap();
            let saved = App::restore_input_for_reload("image-followup-legacy").unwrap();
            let mut restored = create_test_app();
            restored.apply_restored_reload_input(saved);
            assert_eq!(
                restored.queued_messages,
                vec!["staged prompt", "queued prompt"]
            );
            assert!(restored.queued_images.is_empty());
        }
    });
}

#[test]
fn image_followup_recovery_primary_resume_keeps_prompt_routing_and_owned_markers() {
    with_temp_jcode_home(|| {
        for text in ["!echo [image 1]", "/help"] {
            let mut app = create_test_app();
            super::input_images::attach_image(&mut app, "image/png".into(), "aGk=".into());
            super::input::insert_input_text(&mut app, text);
            let prepared = super::input::take_prepared_input(&mut app);
            app.rate_limit_pending_message = Some(PendingRemoteMessage {
                jarvis_voice: false,
                content: prepared.expanded,
                images: prepared.images,
                is_system: false,
                system_reminder: None,
                auto_retry: false,
                retry_attempts: 0,
                retry_at: None,
            });
            let mut restored = reload_image_followup(&app);
            assert!(restored.submit_input_on_startup);
            let prepared = super::input::take_prepared_input(&mut restored);
            assert!(super::input::extract_input_shell_command(prepared.routing_input()).is_none());
            assert!(!prepared.routing_input().trim_start().starts_with('/'));
            assert_eq!(prepared.expanded, text);
            assert_eq!(prepared.images, vec![queued_recovery_image()]);
        }
    });
}

#[test]
fn image_followup_recovery_spawned_startup_prompt_keeps_image_routing() {
    with_temp_jcode_home(|| {
        for text in ["!echo [image 1]", "/help"] {
            App::save_startup_submission_for_session(
                "image-startup-routing",
                text.into(),
                vec![queued_recovery_image()],
            );
            let saved = App::restore_input_for_reload("image-startup-routing").unwrap();
            let mut app = create_test_app();
            app.apply_restored_reload_input(saved);
            let prepared = super::input::take_prepared_input(&mut app);
            assert!(super::input::extract_input_shell_command(prepared.routing_input()).is_none());
            assert!(!prepared.routing_input().trim_start().starts_with('/'));
            assert_eq!(prepared.expanded, text);
            assert_eq!(prepared.images, vec![queued_recovery_image()]);
        }
    });
}
