fn attachment_routing_attach_image(app: &mut App) {
    super::input_images::attach_image(app, "image/png".into(), "fixture-image".into());
}

async fn attachment_routing_read_request(
    reader: &mut (impl tokio::io::AsyncBufRead + Unpin),
) -> serde_json::Value {
    use tokio::io::AsyncBufReadExt;
    loop {
        let mut line = String::new();
        let bytes = tokio::time::timeout(Duration::from_secs(3), reader.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        assert_ne!(bytes, 0, "remote request stream closed");
        let request: serde_json::Value = serde_json::from_str(&line).unwrap();
        if matches!(request["type"].as_str(), Some("message" | "input_shell")) {
            return request;
        }
    }
}

#[test]
fn attachment_routing_pending_reload_keeps_owned_leading_image_intent() {
    with_temp_jcode_home(|| {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            for text in ["!echo hello", "/help", "literal [image 1]"] {
                let mut app = tokio::task::block_in_place(create_test_app);
                app.is_remote = true;
                app.runtime_mode = super::AppRuntimeMode::RemoteClient;
                attachment_routing_attach_image(&mut app);
                super::input::insert_input_text(&mut app, text);
                // Compact paste state must survive with the same owned marker.
                super::input::handle_text_paste(
                    &mut app,
                    " one\ntwo\nthree\nfour\nfive\nsix".into(),
                );
                let raw_input = app.input.clone();
                let prepared = super::input::take_prepared_input(&mut app);
                let expanded = prepared.expanded.clone();
                let pastes = prepared.pasted_contents.clone();
                let markers = serde_json::to_value(&prepared.image_markers).unwrap();
                let mut remote = crate::tui::backend::RemoteConnection::dummy();
                remote.mark_history_loaded();
                let peer = remote.take_dummy_peer().unwrap();
                let (reader, _writer) = peer.into_split();
                let mut reader = tokio::io::BufReader::new(reader);
                super::remote::input_dispatch::submit_prepared_remote_input(
                    &mut app,
                    &mut remote,
                    prepared,
                )
                .await
                .unwrap();
                let sent = attachment_routing_read_request(&mut reader).await;
                assert_eq!(sent["type"], "message");
                assert_eq!(sent["content"], expanded);
                app.save_input_for_reload("attachment-routing-pending");
                let path = crate::storage::jcode_dir()
                    .unwrap()
                    .join("client-input-attachment-routing-pending");
                let saved: serde_json::Value =
                    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
                let restored = App::restore_input_for_reload("attachment-routing-pending").unwrap();
                let mut next = tokio::task::block_in_place(create_test_app);
                next.is_remote = true;
                next.runtime_mode = super::AppRuntimeMode::RemoteClient;
                next.apply_restored_reload_input(restored);
                let mut remote = crate::tui::backend::RemoteConnection::dummy();
                remote.mark_history_loaded();
                let peer = remote.take_dummy_peer().unwrap();
                let (reader, _writer) = peer.into_split();
                let mut reader = tokio::io::BufReader::new(reader);
                super::remote::handle_remote_key(
                    &mut next,
                    KeyCode::Enter,
                    KeyModifiers::NONE,
                    &mut remote,
                )
                .await
                .unwrap();
                let replay = attachment_routing_read_request(&mut reader).await;
                assert_eq!(
                    replay["type"], "message",
                    "reload changed prompt intent: {replay}"
                );
                assert_eq!(replay["content"], expanded);
                assert_eq!(
                    next.rate_limit_pending_message
                        .as_ref()
                        .unwrap()
                        .images
                        .len(),
                    1
                );
                assert_eq!(saved["input"], raw_input);
                assert_eq!(saved["image_markers"], markers);
                assert_eq!(saved["pasted_contents"], serde_json::json!(pastes));
            }
        });
    });
}

#[test]
fn attachment_routing_remote_skill_uses_expanded_image_only_prompt() {
    with_temp_jcode_home(|| {
        let project = tempfile::tempdir().unwrap();
        let skill_dir = project.path().join(".jcode/skills/my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: my-skill\ndescription: Routing fixture\n---\nUse the attached image.\n",
        )
        .unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            for text in ["", "literal [image 1]", "one\ntwo\nthree\nfour\nfive\nsix"] {
                let mut app = tokio::task::block_in_place(create_test_app);
                app.is_remote = true;
                app.runtime_mode = super::AppRuntimeMode::RemoteClient;
                app.session.working_dir = Some(project.path().to_string_lossy().into());
                app.refresh_skills_snapshot();
                super::input::insert_input_text(&mut app, "/my-skill ");
                super::input::handle_text_paste(&mut app, text.into());
                attachment_routing_attach_image(&mut app);
                let prepared = super::input::take_prepared_input(&mut app);
                let mut remote = crate::tui::backend::RemoteConnection::dummy();
                remote.mark_history_loaded();
                let peer = remote.take_dummy_peer().unwrap();
                let (reader, _writer) = peer.into_split();
                let mut reader = tokio::io::BufReader::new(reader);
                super::remote::input_dispatch::submit_remote_slash_input(
                    &mut app,
                    &mut remote,
                    prepared,
                )
                .await
                .unwrap();
                let sent = attachment_routing_read_request(&mut reader).await;
                assert_eq!(sent["type"], "message");
                assert_eq!(
                    sent["content"], text,
                    "owned image marker reached the skill: {sent}"
                );
                assert_eq!(app.active_skill.as_deref(), Some("my-skill"));
                assert_eq!(
                    app.rate_limit_pending_message
                        .as_ref()
                        .unwrap()
                        .images
                        .len(),
                    1
                );
                assert!(!app.pending_turn);
                app.save_input_for_reload("attachment-routing-skill-resume");
                let restored =
                    App::restore_input_for_reload("attachment-routing-skill-resume").unwrap();
                let mut next = tokio::task::block_in_place(create_test_app);
                next.apply_restored_reload_input(restored);
                assert!(
                    next.submit_input_on_startup,
                    "a proven skill draft must retain slash intent"
                );
            }
        });
    });
}

#[test]
fn attachment_routing_true_commands_and_literal_markers_keep_intent() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        for text in ["!echo hello", "/help", "literal [image 1]"] {
            let mut app = tokio::task::block_in_place(create_test_app);
            app.is_remote = true;
            app.runtime_mode = super::AppRuntimeMode::RemoteClient;
            super::input::insert_input_text(&mut app, text);
            attachment_routing_attach_image(&mut app);
            let prepared = super::input::take_prepared_input(&mut app);
            assert_eq!(
                prepared.routing_input().starts_with('!'),
                text.starts_with('!')
            );
            assert_eq!(
                prepared.routing_input().starts_with('/'),
                text.starts_with('/')
            );
            assert_eq!(prepared.expanded.trim_end(), text);
            if text.starts_with('!') {
                let mut remote = crate::tui::backend::RemoteConnection::dummy();
                remote.mark_history_loaded();
                let peer = remote.take_dummy_peer().unwrap();
                let (reader, _writer) = peer.into_split();
                let mut reader = tokio::io::BufReader::new(reader);
                super::remote::input_dispatch::submit_prepared_remote_input(
                    &mut app,
                    &mut remote,
                    prepared,
                )
                .await
                .unwrap();
                let sent = attachment_routing_read_request(&mut reader).await;
                assert_eq!(sent["type"], "input_shell");
            }
        }
    });
}

#[test]
fn attachment_routing_terminal_retry_restores_the_submitted_draft() {
    with_temp_jcode_home(|| {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            for reload_held_retry in [false, true] {
                let mut app = tokio::task::block_in_place(create_test_app);
                app.is_remote = true;
                app.runtime_mode = super::AppRuntimeMode::RemoteClient;
                attachment_routing_attach_image(&mut app);
                super::input::insert_input_text(&mut app, "!echo literal [image 1]");
                let raw_input = app.input.clone();
                let prepared = super::input::take_prepared_input(&mut app);
                let expanded = prepared.expanded.clone();
                let mut remote = crate::tui::backend::RemoteConnection::dummy();
                remote.mark_history_loaded();
                super::remote::input_dispatch::submit_prepared_remote_input(
                    &mut app,
                    &mut remote,
                    prepared,
                )
                .await
                .unwrap();
                if reload_held_retry {
                    app.rate_limit_pending_message.as_mut().unwrap().auto_retry = true;
                    app.rate_limit_reset = Some(Instant::now() + Duration::from_secs(60));
                    app.save_input_for_reload("attachment-routing-held-retry");
                    let restored =
                        App::restore_input_for_reload("attachment-routing-held-retry").unwrap();
                    let mut next = tokio::task::block_in_place(create_test_app);
                    next.apply_restored_reload_input(restored);
                    app = next;
                }
                app.restore_terminal_remote_retry_input();
                assert_eq!(app.input, raw_input);
                let restored = super::input::take_prepared_input(&mut app);
                assert_eq!(restored.expanded, expanded);
                assert_eq!(restored.images.len(), 1);
                assert!(
                    super::input::extract_input_shell_command(restored.routing_input()).is_none()
                );
            }
        });
    });
}

#[test]
fn attachment_routing_pending_request_must_match_the_retained_draft() {
    with_temp_jcode_home(|| {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            for other_payload in [false, true] {
                let mut app = tokio::task::block_in_place(create_test_app);
                app.is_remote = true;
                app.runtime_mode = super::AppRuntimeMode::RemoteClient;
                attachment_routing_attach_image(&mut app);
                super::input::insert_input_text(&mut app, "old prompt");
                let prepared = super::input::take_prepared_input(&mut app);
                let mut remote = crate::tui::backend::RemoteConnection::dummy();
                remote.mark_history_loaded();
                super::remote::input_dispatch::submit_prepared_remote_input(
                    &mut app,
                    &mut remote,
                    prepared,
                )
                .await
                .unwrap();
                if other_payload {
                    app.begin_remote_send(
                        &mut remote,
                        "new ordinary prompt".into(),
                        Vec::new(),
                        false,
                    )
                    .await
                    .unwrap();
                } else {
                    app.current_message_id = Some(app.current_message_id.unwrap() + 1);
                }
                app.save_input_for_reload("attachment-routing-other-request");
                let path = crate::storage::jcode_dir()
                    .unwrap()
                    .join("client-input-attachment-routing-other-request");
                let saved: serde_json::Value =
                    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
                let expected = if other_payload {
                    "new ordinary prompt"
                } else {
                    "old prompt"
                };
                assert_eq!(saved["input"].as_str().unwrap().trim(), expected);
                assert!(saved["last_submitted_draft"].is_null());
                assert_eq!(
                    saved["image_markers"],
                    serde_json::json!({"text":"", "spans":[]})
                );
            }
        });
    });
}

#[test]
fn attachment_routing_legacy_command_shaped_image_retry_requires_review() {
    with_temp_jcode_home(|| {
        let image = serde_json::json!({"media_type":"image/png", "data":"fixture-image"});
        for text in ["!echo hello", "/help"] {
            for metadata in [
                None,
                Some(serde_json::json!({"raw_input":false})),
                Some(serde_json::json!({
                    "raw_input":text, "expanded":text,
                    "images":[["image/png", "fixture-image"]],
                    "image_markers":{"text":text, "spans":[[0,1,1]]},
                    "routing_input":text, "pasted_contents":[],
                })),
            ] {
                let mut saved = serde_json::json!({
                    "input":text, "cursor":text.len(), "pending_images":[image.clone()],
                    "submit_on_restore":true, "pasted_contents":[],
                    "image_markers":{"text":text, "spans":[]},
                });
                if let Some(metadata) = metadata {
                    saved["last_submitted_draft"] = metadata;
                }
                let path = crate::storage::jcode_dir()
                    .unwrap()
                    .join("client-input-attachment-routing-legacy");
                std::fs::write(path, saved.to_string()).unwrap();
                let restored = App::restore_input_for_reload("attachment-routing-legacy").unwrap();
                let mut app = create_test_app();
                app.apply_restored_reload_input(restored);
                assert!(
                    !app.submit_input_on_startup,
                    "unproved image prompt would execute automatically: {text}"
                );
                assert!(app.input.starts_with(text));
                assert_eq!(app.pending_images.len(), 1);
                assert!(app.status_notice.as_ref().unwrap().0.contains("review"));
            }
        }
        // Missing provenance never claims the user's literal marker text.
        let saved = serde_json::json!({
            "input":"literal [image 1]", "pending_images":[image],
            "submit_on_restore":true,
        });
        let path = crate::storage::jcode_dir()
            .unwrap()
            .join("client-input-attachment-routing-literal");
        std::fs::write(path, saved.to_string()).unwrap();
        let restored = App::restore_input_for_reload("attachment-routing-literal").unwrap();
        let mut app = create_test_app();
        app.apply_restored_reload_input(restored);
        assert!(app.submit_input_on_startup);
        let prepared = super::input::take_prepared_input(&mut app);
        assert_eq!(prepared.expanded, "literal [image 1]");
        assert_eq!(prepared.images.len(), 1);
    });
}

#[test]
fn attachment_routing_queued_command_keeps_the_inflight_image_draft() {
    with_temp_jcode_home(|| {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let mut app = tokio::task::block_in_place(create_test_app);
            app.is_remote = true;
            app.runtime_mode = super::AppRuntimeMode::RemoteClient;
            attachment_routing_attach_image(&mut app);
            super::input::insert_input_text(&mut app, "!echo still a prompt");
            let raw_input = app.input.clone();
            let prepared = super::input::take_prepared_input(&mut app);
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            remote.mark_history_loaded();
            super::remote::input_dispatch::submit_prepared_remote_input(
                &mut app,
                &mut remote,
                prepared,
            )
            .await
            .unwrap();
            let request_id = app.current_message_id;
            let next_id = remote.next_request_id_for_test();
            super::input::insert_input_text(&mut app, "/help");
            super::remote::handle_remote_key(
                &mut app,
                KeyCode::Enter,
                KeyModifiers::CONTROL,
                &mut remote,
            )
            .await
            .unwrap();
            assert_eq!(app.queued_commands, vec!["/help"]);
            assert_eq!(app.current_message_id, request_id);
            assert_eq!(remote.next_request_id_for_test(), next_id);
            app.save_input_for_reload("attachment-routing-inflight-queue");
            let path = crate::storage::jcode_dir()
                .unwrap()
                .join("client-input-attachment-routing-inflight-queue");
            let saved: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            assert_eq!(
                saved["input"], raw_input,
                "queue edits discarded the in-flight draft"
            );
            assert_eq!(saved["queued_commands"], serde_json::json!(["/help"]));
        });
    });
}

#[test]
fn startup_prompt_provenance_keeps_literals_whitespace_and_image_order() {
    with_temp_jcode_home(|| {
        let images = vec![
            ("image/png".into(), "aGk=".into()),
            ("image/jpeg".into(), "c2Vjb25k".into()),
        ];
        for text in [
            "!echo [image 1]",
            "/help [image 2]",
            " \t!echo héllo [image 1] \n",
            "見て [image 1]\n[image 2] ",
            "",
        ] {
            App::save_startup_submission_for_session(
                "startup-prompt-provenance",
                text.into(),
                images.clone(),
            );
            let saved = App::restore_input_for_reload("startup-prompt-provenance").unwrap();
            if !text.trim_start().starts_with('!') && !text.trim_start().starts_with('/') {
                assert_eq!(saved.input, text, "ordinary saved text must remain exact");
            }
            let mut app = create_test_app();
            app.apply_restored_reload_input(saved);
            assert!(app.submit_input_on_startup);
            let prepared = super::input::take_prepared_input(&mut app);
            assert!(super::input::extract_input_shell_command(prepared.routing_input()).is_none());
            assert!(!prepared.routing_input().trim_start().starts_with('/'));
            assert_eq!(prepared.expanded, text);
            assert_eq!(prepared.images, images);
        }
    });
}
