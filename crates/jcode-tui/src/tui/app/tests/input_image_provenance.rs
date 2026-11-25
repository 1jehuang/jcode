fn provenance_attach_image(app: &mut App) {
    assert!(
        app.handle_clipboard_paste_completed(crate::bus::ClipboardPasteCompleted {
            session_id: app.session.id.clone(),
            kind: crate::bus::ClipboardPasteKind::Smart,
            content: crate::bus::ClipboardPasteContent::Image {
                media_type: "image/png".into(),
                base64_data: "fixture-image".into(),
            },
        })
    );
}

#[test]
fn image_provenance_literal_marker_cannot_keep_a_deleted_attachment() {
    let mut app = create_test_app();
    super::input::insert_input_text(&mut app, "[image 1] ");
    provenance_attach_image(&mut app);
    app.handle_key(KeyCode::Backspace, KeyModifiers::NONE)
        .unwrap();
    assert!(
        app.pending_images.is_empty(),
        "literal marker kept the deleted payload"
    );
    assert_eq!(app.input, "[image 1] ");
    app.undo_input_change();
    assert_eq!(
        app.pending_images,
        vec![("image/png".into(), "fixture-image".into())]
    );
    assert_eq!(app.input, "[image 1] [image 1]");
}

#[test]
fn image_provenance_stash_carries_attachments_with_its_draft() {
    let mut app = create_test_app();
    provenance_attach_image(&mut app);
    app.toggle_input_stash();
    assert!(app.input.is_empty());
    assert!(
        app.pending_images.is_empty(),
        "stashed image leaked into the new composer"
    );
    super::input::insert_input_text(&mut app, "temporary prompt");
    app.toggle_input_stash();
    assert_eq!(app.input, "[image 1]");
    let prepared = super::input::take_prepared_input(&mut app);
    assert_eq!(
        prepared.images,
        vec![("image/png".into(), "fixture-image".into())]
    );
}

#[test]
fn image_provenance_transport_keeps_literals_and_removes_only_owned_markers() {
    let mut app = create_test_app();
    super::input::insert_input_text(&mut app, "literal [image 1] ");
    provenance_attach_image(&mut app);
    let prepared = super::input::take_prepared_input(&mut app);
    assert_eq!(prepared.expanded, "literal [image 1] ");
    assert_eq!(
        prepared.images,
        vec![("image/png".into(), "fixture-image".into())]
    );
}

#[test]
fn image_provenance_history_navigation_preserves_the_complete_draft() {
    let mut app = create_test_app();
    super::input::insert_input_text(&mut app, "literal [image 1] ");
    provenance_attach_image(&mut app);
    let original = app.input.clone();
    app.persisted_prompt_history = Some(vec!["old prompt".into()]);
    assert!(super::input::handle_prompt_history_navigation(
        &mut app,
        KeyCode::Up,
        KeyModifiers::CONTROL
    ));
    assert!(app.pending_images.is_empty());
    assert!(super::input::handle_prompt_history_navigation(
        &mut app,
        KeyCode::Down,
        KeyModifiers::CONTROL
    ));
    assert_eq!(app.input, original);
    app.handle_key(KeyCode::Backspace, KeyModifiers::NONE)
        .unwrap();
    assert!(app.pending_images.is_empty());
    assert_eq!(app.input, "literal [image 1] ");
}

#[test]
fn image_provenance_legacy_restore_keeps_marker_lookalikes_literal() {
    let mut app = create_test_app();
    super::remote::input_dispatch::restore_prepared_remote_input(
        &mut app,
        super::input::PreparedInput {
            raw_input: "[image 1]".into(),
            expanded: "[image 1]".into(),
            images: vec![("image/png".into(), "fixture-image".into())],
            image_markers: Default::default(),
            routing_input: None,
            pasted_contents: Vec::new(),
        },
    );
    assert_eq!(app.input, "[image 1] [image 1]");
    assert_eq!(app.cursor_pos, app.input.len());
    app.cursor_pos = 0;
    app.handle_key(KeyCode::Delete, KeyModifiers::NONE).unwrap();
    assert_eq!(app.pending_images.len(), 1);
    app.cursor_pos = app.input.len();
    app.handle_key(KeyCode::Backspace, KeyModifiers::NONE)
        .unwrap();
    assert!(app.pending_images.is_empty());
}

#[test]
fn image_provenance_reload_keeps_pastes_cursor_and_marker_association() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        super::input::handle_text_paste(&mut app, "one\ntwo\nthree\nfour\nfive\nsix".into());
        super::input::insert_input_text(&mut app, " [image 1] ");
        provenance_attach_image(&mut app);
        let text = app.input.clone();
        app.cursor_pos = 2;
        app.save_input_for_reload("image-provenance-reload");
        let restored = App::restore_input_for_reload("image-provenance-reload").unwrap();
        let mut next = create_test_app();
        next.apply_restored_reload_input(restored);
        assert_eq!(next.input, text);
        assert_eq!(next.cursor_pos, 2);
        next.cursor_pos = next.input.len();
        next.handle_key(KeyCode::Backspace, KeyModifiers::NONE)
            .unwrap();
        assert!(next.pending_images.is_empty());
        next.undo_input_change();
        let prepared = super::input::take_prepared_input(&mut next);
        assert!(
            prepared
                .expanded
                .contains("one\ntwo\nthree\nfour\nfive\nsix")
        );
        assert_eq!(prepared.expanded.matches("[image 1]").count(), 1);
        assert_eq!(prepared.images.len(), 1);
    });
}

#[test]
fn image_provenance_prepared_command_and_voice_roundtrips_keep_exact_ownership() {
    let mut app = create_test_app();
    super::input::insert_input_text(&mut app, "literal [image 1] ");
    provenance_attach_image(&mut app);
    let original = app.input.clone();
    let prepared = super::input::take_prepared_input(&mut app);
    super::remote::input_dispatch::restore_prepared_remote_input(&mut app, prepared);
    let draft = super::input::swap_in_queued_command(&mut app, "/help".into());
    app.input.clear();
    super::input::restore_composer_draft(&mut app, draft);
    assert_eq!(app.input, original);
    super::remote::submit_voice_transcript(&mut app, "voice message");
    assert_eq!(app.input, original);
    app.handle_key(KeyCode::Backspace, KeyModifiers::NONE)
        .unwrap();
    assert_eq!(app.input, "literal [image 1] ");
    assert!(app.pending_images.is_empty());
}

#[test]
fn image_provenance_unicode_edits_and_multi_digit_renumbering_keep_payloads() {
    let mut app = create_test_app();
    for n in 0..12 {
        super::input_images::attach_image(&mut app, "image/png".into(), format!("payload-{n}"));
    }
    app.cursor_pos = 0;
    super::input::insert_input_text(&mut app, "é图🌈");
    app.handle_key(KeyCode::Delete, KeyModifiers::NONE).unwrap();
    assert_eq!(app.pending_images.len(), 11);
    assert_eq!(app.pending_images[0].1, "payload-1");
    assert!(app.input.starts_with("é图🌈[image 1]"));
    assert!(app.input.ends_with("[image 11]"));
    app.undo_input_change();
    assert_eq!(app.pending_images.len(), 12);
    assert_eq!(app.pending_images[0].1, "payload-0");
}

#[test]
fn image_provenance_invalid_metadata_never_claims_literal_text() {
    let mut app = create_test_app();
    app.input = "[image 1]".into();
    app.pending_images = vec![("image/png".into(), "fixture-image".into())];
    app.input_image_markers = serde_json::from_value(serde_json::json!({
        "text": app.input, "spans": [[0, 9, 1], [0, 9, 1]]
    }))
    .unwrap();
    app.ensure_input_image_markers();
    assert_eq!(app.input, "[image 1] [image 1]");
    let prepared = super::input::take_prepared_input(&mut app);
    assert_eq!(prepared.expanded, "[image 1]");
    assert_eq!(prepared.images.len(), 1);
}

#[test]
fn image_provenance_direct_submit_clears_composer_and_sends_only_literal_marker_text() {
    let mut app = create_test_app();
    super::input::insert_input_text(&mut app, "literal [image 1] ");
    provenance_attach_image(&mut app);
    app.submit_input();
    assert!(app.input.is_empty());
    assert!(app.pending_images.is_empty());
    let message = app.session.messages.last().unwrap();
    assert!(
        message.content.iter().any(
            |block| matches!(block, ContentBlock::Image { data, .. } if data == "fixture-image")
        )
    );
    let text: String = message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "literal [image 1] ");
}

#[test]
fn image_provenance_partial_literal_and_owned_edits_keep_payload_identity() {
    for key in [KeyCode::Backspace, KeyCode::Delete, KeyCode::Char('x')] {
        let mut app = create_test_app();
        super::input::insert_input_text(&mut app, "[image 1] ");
        provenance_attach_image(&mut app);
        let original = app.input.clone();
        app.cursor_pos = 4;
        app.handle_key(key, KeyModifiers::NONE).unwrap();
        assert_eq!(app.pending_images.len(), 1);
        app.undo_input_change();
        assert_eq!(app.input, original);
        app.cursor_pos = "[image 1] ".len() + 4;
        app.handle_key(key, KeyModifiers::NONE).unwrap();
        assert!(app.pending_images.is_empty());
        app.undo_input_change();
        assert_eq!(app.input, original);
        assert_eq!(app.pending_images.len(), 1);
    }
}

#[test]
fn image_provenance_repeated_paste_and_backslash_newline_rebase_owned_spans() {
    for image_first in [false, true] {
        let mut app = create_test_app();
        let text = "é\nβ\n图\n🌈\n[image 1]";
        if image_first {
            provenance_attach_image(&mut app);
        }
        super::input::handle_text_paste(&mut app, text.into());
        if !image_first {
            provenance_attach_image(&mut app);
        }
        super::input::handle_text_paste(&mut app, text.into());
        app.cursor_pos = app.input.len();
        super::input::insert_input_text(&mut app, "\\");
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        let prepared = super::input::take_prepared_input(&mut app);
        assert_eq!(prepared.expanded, format!("{text}\n"));
        assert_eq!(prepared.images.len(), 1);
    }
}

#[test]
fn image_provenance_owned_prefix_keeps_shell_and_slash_text_as_remote_prompts() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    for text in ["!echo must stay a prompt", "/help", "/model"] {
        let mut app = create_test_app();
        provenance_attach_image(&mut app);
        super::input::insert_input_text(&mut app, text);
        app.is_remote = true;
        app.runtime_mode = super::AppRuntimeMode::RemoteClient;
        rt.block_on(async {
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            remote.mark_history_loaded();
            app.handle_remote_key(KeyCode::Enter, KeyModifiers::NONE, &mut remote)
                .await
                .unwrap();
        });
        let pending = app
            .rate_limit_pending_message
            .as_ref()
            .expect("must send an ordinary prompt");
        assert_eq!(pending.content, text);
        assert_eq!(pending.images.len(), 1);
        assert!(app.help_scroll.is_none());
    }
}

#[test]
fn image_provenance_implicit_restore_marker_preserves_context_command_intent() {
    let mut app = create_test_app();
    app.input = "/context".into();
    app.pending_images = vec![("image/png".into(), "fixture-image".into())];
    let prepared = super::input::take_prepared_input(&mut app);
    assert_eq!(prepared.routing_input(), "/context");
    assert_eq!(prepared.raw_input, "/context [image 1]");
    app.is_remote = true;
    app.runtime_mode = super::AppRuntimeMode::RemoteClient;
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        super::remote::submit_remote_slash_input(&mut app, &mut remote, prepared)
            .await
            .unwrap();
    });
    assert_eq!(
        app.display_messages.last().unwrap().title.as_deref(),
        Some("Context")
    );
    assert_eq!(app.pending_images.len(), 1);
    assert!(!app.pending_turn);
}

#[test]
fn image_provenance_deferred_failure_keeps_collapsed_paste_content_recoverable() {
    let mut app = create_test_app();
    let text = "first\nsecond\nthird\nfourth\nfifth\nsixth";
    super::input::handle_text_paste(&mut app, text.into());
    provenance_attach_image(&mut app);
    let prepared = super::input::take_prepared_input(&mut app);
    app.is_remote = true;
    app.runtime_mode = super::AppRuntimeMode::RemoteClient;
    app.auto_poke_incomplete_todos = false;
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        super::remote::submit_prepared_remote_input(&mut app, &mut remote, prepared)
            .await
            .unwrap();
        super::input::insert_input_text(&mut app, "newer draft");
        remote.mark_history_loaded();
        drop(remote.take_dummy_peer());
        super::remote::process_remote_followups(&mut app, &mut remote).await;
    });
    assert!(app.input.starts_with("newer draft"));
    assert_eq!(app.pending_images.len(), 1);
    assert!(
        app.display_messages
            .iter()
            .any(|message| message.content.contains(text))
    );
}

#[test]
fn image_provenance_cut_clears_full_draft_and_undo_restores_it() {
    let mut app = create_test_app();
    super::input::handle_text_paste(&mut app, "one\ntwo\nthree\nfour\nfive\nsix".into());
    provenance_attach_image(&mut app);
    let original = app.input.clone();
    assert!(!super::input::cut_input_line_to_clipboard_with(
        &mut app,
        |_| false
    ));
    assert_eq!(app.input, original);
    assert_eq!(app.pending_images.len(), 1);
    assert_eq!(app.pasted_contents.len(), 1);
    assert!(super::input::cut_input_line_to_clipboard_with(
        &mut app,
        |text| {
            assert_eq!(text, original);
            true
        }
    ));
    assert!(app.input.is_empty());
    assert!(app.pending_images.is_empty());
    assert!(app.pasted_contents.is_empty());
    app.undo_input_change();
    assert_eq!(app.input, original);
    assert_eq!(app.pending_images.len(), 1);
    assert_eq!(
        app.pasted_contents,
        vec!["one\ntwo\nthree\nfour\nfive\nsix"]
    );
}
