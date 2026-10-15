// Literal marker-looking text is never an attachment ownership token.
#[test]
fn image_provenance_quit_clear_stash_undo_preserves_literal_association() {
    for before in [true, false] {
        let (mut app, png, owned, literal) = provenance_collision_draft(before);
        super::input::clear_draft_or_request_quit(&mut app);
        assert!(app.input.is_empty());
        assert!(app.pending_images.is_empty());
        app.undo_input_change();
        assert_provenance_collision_edit(app, png, owned, literal);
    }
}
#[test]
fn image_provenance_collapsed_paste_fallback_retains_command_and_png() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    for command in ["/help", "/definitely-no-provenance-skill"] {
        let mut app = create_test_app();
        super::input::handle_text_paste(&mut app, format!("{command} {}", "argument\n".repeat(6)));
        assert_eq!(app.pasted_contents.len(), 1);
        let png = paste_selection_test_png(&mut app, 86);
        let prepared = super::input::take_prepared_input(&mut app);
        app.is_remote = true;
        app.runtime_mode = super::AppRuntimeMode::RemoteClient;
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();
        rt.block_on(super::remote::submit_remote_slash_input(&mut app, &mut remote, prepared)).unwrap();
        assert_eq!(app.display_messages().last().unwrap().role, "error");
        assert!(app.queued_messages.is_empty(), "collapsed command became a model prompt");
        assert_eq!(app.pending_images, vec![("image/png".into(), png)]);
    }
}
#[test]
fn image_provenance_remote_owned_prefix_does_not_activate_commands() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let temp = tempfile::tempdir().unwrap();
    let skill_dir = temp.path().join(".jcode/skills/provenance-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "---\nname: provenance-skill\ndescription: Marker routing fixture\n---\nKeep prompt text.\n").unwrap();
    for text in ["!echo must-remain-prompt", "/help", "/model", "/provenance-skill prompt"] {
        let mut app = create_test_app();
        let png = paste_selection_test_png(&mut app, 85);
        app.session.working_dir = Some(temp.path().to_string_lossy().to_string());
        app.refresh_skills_snapshot();
        assert!(app.current_skills_snapshot().get("provenance-skill").is_some());
        app.is_remote = true;
        app.runtime_mode = super::AppRuntimeMode::RemoteClient;
        super::input::insert_input_text(&mut app, text);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();
        rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote)).unwrap();
        let pending = app.rate_limit_pending_message.as_ref().expect("must send a prompt, not run a command");
        assert_eq!(pending.content, text);
        assert_eq!(pending.images, vec![("image/png".into(), png)]);
        assert!(app.help_scroll.is_none());
        assert!(app.active_skill.is_none());
    }
}
#[test]
fn image_provenance_stash_and_escape_undo_preserve_exact_association() {
    for before in [true, false] {
        let (mut app, png, owned, literal) = provenance_collision_draft(before);
        app.toggle_input_stash();
        assert!(app.input.is_empty());
        assert!(app.pending_images.is_empty());
        super::input::insert_input_text(&mut app, "temporary");
        app.toggle_input_stash();
        super::input::clear_input_for_escape(&mut app);
        assert!(app.pending_images.is_empty());
        app.undo_input_change();
        assert_provenance_collision_edit(app, png, owned, literal);
    }
}

#[test]
fn image_provenance_literal_partial_edits_do_not_damage_owned_png() {
    for key in [KeyCode::Backspace, KeyCode::Delete, KeyCode::Char('x')] {
        let (mut app, png, owned, literal) = provenance_collision_draft(true);
        let original = app.input.clone();
        app.cursor_pos = literal + 4;
        app.handle_key(key, KeyModifiers::empty()).unwrap();
        assert_eq!(app.pending_images, vec![("image/png".into(), png.clone())]);
        app.undo_input_change();
        assert_eq!(app.input, original);
        app.cursor_pos = owned + 4;
        app.handle_key(key, KeyModifiers::empty()).unwrap();
        assert!(app.pending_images.is_empty());
        app.undo_input_change();
        assert_provenance_collision_edit(app, png, owned, literal);
    }
}

#[test]
fn image_provenance_content_only_startup_and_interleave_keep_literals() {
    with_temp_jcode_home(|| {
        for disk in [true, false] {
            let (mut app, png, _, _) = provenance_collision_draft(true);
            let prepared = super::input::take_prepared_input(&mut app);
            assert_eq!(prepared.expanded.matches("[image 1]").count(), 1);
            if disk {
                App::save_startup_submission_for_session(
                    "literal-transfer",
                    prepared.expanded,
                    prepared.images,
                );
                let restored = App::restore_input_for_reload("literal-transfer").unwrap();
                app.apply_restored_reload_input(restored);
            } else {
                super::input::stage_local_interleave(&mut app, prepared.expanded, prepared.images);
                assert!(super::input::retrieve_pending_message_for_edit(&mut app));
            }
            let owned = app.input.rfind("[image 1]").unwrap();
            assert_provenance_collision_edit(app, png, owned, 0);
        }
    });
}
#[test]
fn image_provenance_history_roundtrip_keeps_literal_and_payload_association() {
    let (mut app, png, owned, literal) = provenance_collision_draft(true);
    app.persisted_prompt_history = Some(vec!["older prompt".into()]);
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
    assert_provenance_collision_edit(app, png, owned, literal);
}

#[test]
fn image_provenance_malformed_metadata_is_not_used_to_claim_literals() {
    let (mut app, png, _, _) = provenance_collision_draft(true);
    app.input_image_markers = serde_json::from_value(serde_json::json!({
        "text": app.input,
        "spans": [[0, 9, 1], [0, 9, 1]]
    }))
    .unwrap();
    app.ensure_input_image_markers();
    assert_eq!(app.input.matches("[image 1]").count(), 3);
    app.set_input_selection(0, 9);
    app.handle_key(KeyCode::Delete, KeyModifiers::empty())
        .unwrap();
    assert_eq!(app.pending_images, vec![("image/png".into(), png)]);
}

#[test]
fn image_provenance_backslash_newline_preserves_owned_marker() {
    let (mut app, png, owned, literal) = provenance_collision_draft(true);
    super::input::insert_input_text(&mut app, "\\");
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();
    assert_eq!(app.input.matches("[image 1]").count(), 2);
    assert_provenance_collision_edit(app, png, owned, literal);
}

#[test]
fn image_provenance_voice_roundtrip_preserves_literal_association() {
    let (mut app, png, owned, literal) = provenance_collision_draft(true);
    super::remote::submit_voice_transcript(&mut app, "voice message");
    assert_provenance_collision_edit(app, png, owned, literal);
}

#[test]
fn image_provenance_direct_submit_transports_literal_but_not_owned_marker() {
    let (mut app, png, _, _) = provenance_collision_draft(true);
    app.submit_input();
    let message = app.session.messages.last().unwrap();
    let text: String = message
        .content
        .iter()
        .filter_map(|block| match block {
            crate::message::ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text.matches("[image 1]").count(), 1);
    assert!(message.content.iter().any(|block| matches!(block,
        crate::message::ContentBlock::Image { data, .. } if data == &png)));
}

#[test]
fn image_provenance_owned_prefix_does_not_expose_slash_command_to_dispatch() {
    let mut app = create_test_app();
    paste_selection_test_png(&mut app, 83);
    super::input::insert_input_text(&mut app, "/help");
    app.submit_input();
    assert!(app.is_processing);
    assert!(app.help_scroll.is_none());
}

fn provenance_collision_draft(literal_before: bool) -> (App, String, usize, usize) {
    let mut app = create_test_app();
    if literal_before {
        super::input::insert_input_text(&mut app, "[image 1] ");
    }
    let owned = app.cursor_pos;
    let png = paste_selection_test_png(&mut app, 81);
    let literal = if literal_before {
        0
    } else {
        super::input::insert_input_text(&mut app, " [image 1]");
        "[image 1] ".len()
    };
    (app, png, owned, literal)
}

fn assert_provenance_collision_edit(mut app: App, png: String, owned: usize, literal: usize) {
    let original = app.input.clone();
    app.set_input_selection(literal, literal + "[image 1]".len());
    app.handle_key(KeyCode::Delete, KeyModifiers::empty())
        .unwrap();
    assert_eq!(app.pending_images, vec![("image/png".into(), png.clone())]);
    app.undo_input_change();
    assert_eq!(app.input, original);
    assert_eq!(app.pending_images, vec![("image/png".into(), png.clone())]);
    app.set_input_selection(owned, owned + "[image 1]".len());
    app.handle_key(KeyCode::Backspace, KeyModifiers::empty())
        .unwrap();
    assert!(app.pending_images.is_empty());
    assert!(app.input.contains("[image 1]"), "literal text was removed");
    app.undo_input_change();
    assert_eq!(app.input, original);
    assert_eq!(app.pending_images, vec![("image/png".into(), png)]);
}

#[test]
fn image_provenance_literals_before_and_after_owned_marker_survive_edit_and_undo() {
    for before in [true, false] {
        let (app, png, owned, literal) = provenance_collision_draft(before);
        assert_provenance_collision_edit(app, png, owned, literal);
    }
}

#[test]
fn image_provenance_prepared_and_queued_draft_restores_keep_literal_distinct() {
    for before in [true, false] {
        let (mut app, png, owned, literal) = provenance_collision_draft(before);
        let prepared = super::input::take_prepared_input(&mut app);
        super::remote::restore_prepared_remote_input(&mut app, prepared);
        let draft = super::input::swap_in_queued_command(&mut app, "/help".into());
        app.input.clear();
        super::input::restore_composer_draft(&mut app, draft);
        assert_provenance_collision_edit(app, png, owned, literal);
    }
}

#[test]
fn image_provenance_disk_reload_retains_owned_marker_association() {
    with_temp_jcode_home(|| {
        for before in [true, false] {
            let (app, png, owned, literal) = provenance_collision_draft(before);
            app.save_input_for_reload("provenance-draft");
            let restored = App::restore_input_for_reload("provenance-draft").unwrap();
            let mut next = create_test_app();
            next.apply_restored_reload_input(restored);
            assert_provenance_collision_edit(next, png, owned, literal);
        }
    });
}

#[test]
fn image_provenance_legacy_restore_does_not_claim_literal_marker() {
    let mut app = create_test_app();
    let png = paste_selection_test_png(&mut app, 82);
    let mut next = create_test_app();
    super::remote::restore_prepared_remote_input(
        &mut next,
        super::input::PreparedInput {
            image_markers: Default::default(),
            routing_input: None,
            pasted_contents: Vec::new(),
            raw_input: "[image 1]".into(),
            expanded: "[image 1]".into(),
            images: vec![("image/png".into(), png.clone())],
        },
    );
    assert_eq!(
        next.input, "[image 1] [image 1]",
        "legacy text remains literal, a fresh owned marker is appended"
    );
    next.set_input_selection(0, "[image 1]".len());
    next.handle_key(KeyCode::Delete, KeyModifiers::empty())
        .unwrap();
    assert_eq!(next.pending_images, vec![("image/png".into(), png)]);
}
