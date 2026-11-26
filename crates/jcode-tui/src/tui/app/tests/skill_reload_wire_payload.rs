const SKILL_RELOAD_PASTE: &str = "one\ntwo\nthree\nfour\nfive\nsix";

fn skill_reload_write_skill(project: &std::path::Path, name: &str) {
    let directory = project.join(".jcode/skills").join(name);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: Reload wire fixture\n---\nUse this skill.\n"),
    )
    .unwrap();
}

fn skill_reload_app(project: &std::path::Path) -> App {
    let mut app = create_test_app();
    app.is_remote = true;
    app.runtime_mode = super::AppRuntimeMode::RemoteClient;
    app.auto_poke_incomplete_todos = false;
    app.session.working_dir = Some(project.to_string_lossy().into());
    app.refresh_skills_snapshot();
    app
}

fn skill_reload_assert_wire(original: &serde_json::Value, replay: &serde_json::Value) {
    assert_eq!(replay["type"], "message");
    for field in [
        "content",
        "images",
        "active_skill",
        "system_reminder",
        "jarvis_voice",
    ] {
        assert_eq!(
            replay[field], original[field],
            "reload changed wire field {field}"
        );
    }
}

async fn skill_reload_submit(
    app: &mut App,
    remote: &mut crate::tui::backend::RemoteConnection,
    name: &str,
    text: &str,
) -> super::input_images::InputUndoSnapshot {
    super::input::insert_input_text(app, &format!("/{name} "));
    super::input::handle_text_paste(app, text.into());
    attachment_routing_attach_image(app);
    let draft = app.input_draft_snapshot();
    let prepared = super::input::take_prepared_input(app);
    super::remote::input_dispatch::submit_remote_slash_input(app, remote, prepared)
        .await
        .unwrap();
    draft
}

#[test]
fn skill_reload_wire_startup_replays_skill_payload_and_activation() {
    with_temp_jcode_home(|| {
        let project = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            for name in ["my-skill", "my skill"] {
                skill_reload_write_skill(project.path(), name);
                for text in [
                    "",
                    "literal [image 1] and [Paste #99: 6 lines]",
                    "!echo literal shell intent",
                    SKILL_RELOAD_PASTE,
                ] {
                    let mut app = tokio::task::block_in_place(|| skill_reload_app(project.path()));
                    let mut remote = crate::tui::backend::RemoteConnection::dummy();
                    remote.mark_history_loaded();
                    let peer = remote.take_dummy_peer().unwrap();
                    let (reader, _writer) = peer.into_split();
                    let mut reader = tokio::io::BufReader::new(reader);
                    let draft = skill_reload_submit(&mut app, &mut remote, name, text).await;
                    let original = attachment_routing_read_request(&mut reader).await;
                    assert_eq!(original["content"], text);
                    assert_eq!(original["active_skill"], name);
                    app.save_input_for_reload("skill-wire-startup");
                    let restored = App::restore_input_for_reload("skill-wire-startup").unwrap();
                    let mut next = tokio::task::block_in_place(|| skill_reload_app(project.path()));
                    next.apply_restored_reload_input(restored);
                    assert_eq!(next.input_draft_snapshot(), draft);
                    assert!(next.submit_input_on_startup);
                    assert!(next.active_skill.is_none());
                    super::remote::process_remote_followups(&mut next, &mut remote).await;
                    let replay = attachment_routing_read_request(&mut reader).await;
                    skill_reload_assert_wire(&original, &replay);
                    assert_eq!(next.active_skill.as_deref(), Some(name));
                    next.restore_terminal_remote_retry_input();
                    assert_eq!(next.input_draft_snapshot(), draft);
                    next.cursor_pos = next.input.len();
                    next.handle_key(KeyCode::Backspace, KeyModifiers::NONE)
                        .unwrap();
                    assert!(next.pending_images.is_empty());
                    next.undo_input_change();
                    assert_eq!(next.input_draft_snapshot(), draft);
                }
            }
        });
    });
}

#[test]
fn skill_reload_wire_held_retry_preserves_activation_and_newer_composer() {
    with_temp_jcode_home(|| {
        let project = tempfile::tempdir().unwrap();
        skill_reload_write_skill(project.path(), "my skill");
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let mut app = tokio::task::block_in_place(|| skill_reload_app(project.path()));
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            remote.mark_history_loaded();
            let peer = remote.take_dummy_peer().unwrap();
            let (reader, _writer) = peer.into_split();
            let mut reader = tokio::io::BufReader::new(reader);
            let original_draft =
                skill_reload_submit(&mut app, &mut remote, "my skill", SKILL_RELOAD_PASTE).await;
            let original = attachment_routing_read_request(&mut reader).await;
            retry_draft_rate_error(&mut app, &mut remote);
            app.rate_limit_pending_message.as_mut().unwrap().auto_retry = true;
            super::input::insert_input_text(&mut app, "newer literal [image 1] ");
            super::input::handle_text_paste(&mut app, SKILL_RELOAD_PASTE.into());
            attachment_routing_attach_image(&mut app);
            let newer = app.input_draft_snapshot();
            app.save_input_for_reload("skill-wire-held");
            let restored = App::restore_input_for_reload("skill-wire-held").unwrap();
            let mut next = tokio::task::block_in_place(|| skill_reload_app(project.path()));
            next.apply_restored_reload_input(restored);
            assert!(!next.submit_input_on_startup);
            assert_eq!(next.input_draft_snapshot(), newer);
            for _ in 0..2 {
                retry_draft_dispatch(&mut next, &mut remote).await;
                let replay = attachment_routing_read_request(&mut reader).await;
                skill_reload_assert_wire(&original, &replay);
                assert_eq!(next.input_draft_snapshot(), newer);
                assert_eq!(
                    next.last_submitted_draft.as_ref().unwrap().1.raw_input,
                    original_draft.0
                );
                retry_draft_rate_error(&mut next, &mut remote);
            }
        });
    });
}

#[test]
fn skill_reload_wire_missing_or_changed_catalog_pauses_for_review() {
    with_temp_jcode_home(|| {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            for held in [false, true] {
                for changed in [false, true] {
                    let project = tempfile::tempdir().unwrap();
                    skill_reload_write_skill(project.path(), "my");
                    let mut app = tokio::task::block_in_place(|| skill_reload_app(project.path()));
                    let mut remote = crate::tui::backend::RemoteConnection::dummy();
                    remote.mark_history_loaded();
                    let peer = remote.take_dummy_peer().unwrap();
                    let (reader, _writer) = peer.into_split();
                    let mut reader = tokio::io::BufReader::new(reader);
                    let draft =
                        skill_reload_submit(&mut app, &mut remote, "my", "skill payload").await;
                    attachment_routing_read_request(&mut reader).await;
                    if held {
                        retry_draft_rate_error(&mut app, &mut remote);
                        app.rate_limit_pending_message.as_mut().unwrap().auto_retry = true;
                    }
                    app.save_input_for_reload("skill-wire-catalog");
                    if changed {
                        skill_reload_write_skill(project.path(), "my skill");
                    } else {
                        std::fs::remove_dir_all(project.path().join(".jcode/skills/my")).unwrap();
                    }
                    let restored = App::restore_input_for_reload("skill-wire-catalog").unwrap();
                    let mut next = tokio::task::block_in_place(|| skill_reload_app(project.path()));
                    next.apply_restored_reload_input(restored);
                    if held {
                        retry_draft_dispatch(&mut next, &mut remote).await;
                    } else {
                        super::remote::process_remote_followups(&mut next, &mut remote).await;
                    }
                    assert!(
                        !next.is_processing,
                        "a changed skill must not replay under different semantics"
                    );
                    assert!(!next.submit_input_on_startup);
                    assert!(next.rate_limit_pending_message.is_none());
                    assert!(next.rate_limit_reset.is_none());
                    assert_eq!(next.input_draft_snapshot(), draft);
                    assert!(
                        next.status_notice
                            .as_ref()
                            .unwrap()
                            .0
                            .contains("needs review")
                    );
                    assert!(
                        tokio::time::timeout(
                            Duration::from_millis(50),
                            attachment_routing_read_request(&mut reader)
                        )
                        .await
                        .is_err()
                    );
                }
            }
        });
    });
}

#[test]
fn skill_reload_wire_ordinary_slash_and_owned_prefix_never_become_new_skills() {
    with_temp_jcode_home(|| {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            for owned_prefix in [false, true] {
                let project = tempfile::tempdir().unwrap();
                let mut app = tokio::task::block_in_place(|| skill_reload_app(project.path()));
                if owned_prefix {
                    attachment_routing_attach_image(&mut app);
                }
                super::input::insert_input_text(&mut app, "/later-skill !echo literal [image 1] ");
                super::input::handle_text_paste(&mut app, SKILL_RELOAD_PASTE.into());
                if !owned_prefix {
                    attachment_routing_attach_image(&mut app);
                }
                let draft = app.input_draft_snapshot();
                let prepared = super::input::take_prepared_input(&mut app);
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
                let original = attachment_routing_read_request(&mut reader).await;
                assert!(original["active_skill"].is_null());
                app.save_input_for_reload("skill-wire-ordinary");
                skill_reload_write_skill(project.path(), "later-skill");
                let restored = App::restore_input_for_reload("skill-wire-ordinary").unwrap();
                let mut next = tokio::task::block_in_place(|| skill_reload_app(project.path()));
                next.apply_restored_reload_input(restored);
                super::remote::process_remote_followups(&mut next, &mut remote).await;
                let replay = attachment_routing_read_request(&mut reader).await;
                skill_reload_assert_wire(&original, &replay);
                assert!(next.active_skill.is_none());
                next.restore_terminal_remote_retry_input();
                assert_eq!(next.input_draft_snapshot(), draft);
            }
        });
    });
}

#[test]
fn skill_reload_wire_failed_write_retains_full_draft_and_new_intent_never_uses_old_payload() {
    with_temp_jcode_home(|| {
        let project = tempfile::tempdir().unwrap();
        skill_reload_write_skill(project.path(), "my-skill");
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            for replace in [false, true] {
                let mut app = tokio::task::block_in_place(|| skill_reload_app(project.path()));
                let mut remote = crate::tui::backend::RemoteConnection::dummy();
                remote.mark_history_loaded();
                let original =
                    skill_reload_submit(&mut app, &mut remote, "my-skill", SKILL_RELOAD_PASTE)
                        .await;
                app.save_input_for_reload("skill-wire-write");
                let restored = App::restore_input_for_reload("skill-wire-write").unwrap();
                let mut next = tokio::task::block_in_place(|| skill_reload_app(project.path()));
                next.apply_restored_reload_input(restored);
                let mut connection = crate::tui::backend::RemoteConnection::dummy();
                connection.mark_history_loaded();
                if replace {
                    super::input::clear_input_for_escape(&mut next);
                    super::input::insert_input_text(&mut next, "new ordinary intent");
                    let peer = connection.take_dummy_peer().unwrap();
                    let (reader, _writer) = peer.into_split();
                    let mut reader = tokio::io::BufReader::new(reader);
                    super::remote::process_remote_followups(&mut next, &mut connection).await;
                    let sent = attachment_routing_read_request(&mut reader).await;
                    assert_eq!(sent["content"], "new ordinary intent");
                    assert!(sent["active_skill"].is_null());
                    assert!(sent["images"].as_array().is_none_or(Vec::is_empty));
                } else {
                    drop(connection.take_dummy_peer());
                    super::remote::process_remote_followups(&mut next, &mut connection).await;
                    assert_eq!(next.input_draft_snapshot(), original);
                    assert!(!next.is_processing);
                }
            }
        });
    });
}

#[test]
fn skill_reload_wire_catalog_pause_preserves_newer_composer() {
    with_temp_jcode_home(|| {
        let project = tempfile::tempdir().unwrap();
        skill_reload_write_skill(project.path(), "my-skill");
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let mut app = tokio::task::block_in_place(|| skill_reload_app(project.path()));
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            remote.mark_history_loaded();
            let peer = remote.take_dummy_peer().unwrap();
            let (reader, _writer) = peer.into_split();
            let mut reader = tokio::io::BufReader::new(reader);
            skill_reload_submit(&mut app, &mut remote, "my-skill", SKILL_RELOAD_PASTE).await;
            attachment_routing_read_request(&mut reader).await;
            retry_draft_rate_error(&mut app, &mut remote);
            app.rate_limit_pending_message.as_mut().unwrap().auto_retry = true;
            super::input::insert_input_text(&mut app, "newer literal [image 1] ");
            super::input::handle_text_paste(&mut app, "new1\nnew2\nnew3\nnew4\nnew5\nnew6".into());
            super::input_images::attach_image(
                &mut app,
                "image/png".into(),
                "catalog-newer-image".into(),
            );
            let newer = app.input_draft_snapshot();
            app.save_input_for_reload("skill-wire-newer-catalog");
            std::fs::remove_dir_all(project.path().join(".jcode/skills/my-skill")).unwrap();
            let restored = App::restore_input_for_reload("skill-wire-newer-catalog").unwrap();
            let mut next = tokio::task::block_in_place(|| skill_reload_app(project.path()));
            next.apply_restored_reload_input(restored);
            retry_draft_dispatch(&mut next, &mut remote).await;
            assert!(!next.is_processing);
            assert!(next.rate_limit_pending_message.is_none());
            assert!(next.input.starts_with(&newer.0));
            assert_eq!(next.cursor_pos, newer.1);
            assert_eq!(&next.pasted_contents, newer.4.as_ref());
            assert_eq!(next.pending_images.len(), 2);
            assert_eq!(next.pending_images[0].1, "catalog-newer-image");
            assert_eq!(next.pending_images[1].1, "fixture-image");
            assert!(
                next.display_messages()
                    .iter()
                    .any(|message| message.content.contains(SKILL_RELOAD_PASTE))
            );
            assert!(
                tokio::time::timeout(
                    Duration::from_millis(50),
                    attachment_routing_read_request(&mut reader)
                )
                .await
                .is_err()
            );
            next.cursor_pos = next.input.len();
            let recovered = next.input_draft_snapshot();
            next.handle_key(KeyCode::Backspace, KeyModifiers::NONE)
                .unwrap();
            next.undo_input_change();
            assert_eq!(next.input_draft_snapshot(), recovered);
        });
    });
}
