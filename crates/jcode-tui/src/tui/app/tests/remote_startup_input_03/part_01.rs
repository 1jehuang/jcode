#[test]
fn test_build_turn_footer_combines_compact_duration_with_streaming_stats() {
    let mut app = create_test_app();
    app.streaming.streaming_input_tokens = 210_000;
    app.streaming.streaming_output_tokens = 440;
    app.streaming.streaming_tps_collect_output = true;
    app.streaming.streaming_total_output_tokens = 440;
    app.streaming.streaming_tps_observed_output_tokens = 440;
    app.streaming.streaming_tps_observed_elapsed = Duration::from_secs(220);

    let footer = app
        .build_turn_footer(Some(316.1))
        .expect("footer with stats");

    assert!(
        footer.starts_with("5m 16s · "),
        "unexpected footer: {footer}"
    );
    assert!(footer.contains(" tps"), "unexpected footer: {footer}");
    assert!(
        footer.ends_with("↑210k ↓440"),
        "unexpected footer: {footer}"
    );
}

#[test]
fn test_processing_status_display() {
    let status = ProcessingStatus::Sending;
    assert!(matches!(status, ProcessingStatus::Sending));

    let status = ProcessingStatus::Streaming;
    assert!(matches!(status, ProcessingStatus::Streaming));

    let status = ProcessingStatus::RunningTool("bash".to_string());
    if let ProcessingStatus::RunningTool(name) = status {
        assert_eq!(name, "bash");
    } else {
        panic!("Expected RunningTool");
    }
}

#[test]
fn test_skill_invocation_not_queued() {
    let mut app = create_test_app();

    // Type a slash invocation for a skill that does not exist. The name must
    // not collide with a built-in slash command (`/test` is the verification
    // orchestrator now), so use an obviously bogus skill name.
    for ch in "/nosuchskill".chars() {
        app.handle_key(KeyCode::Char(ch), KeyModifiers::empty())
            .unwrap();
    }

    app.submit_input();

    // Should show error for unknown skill, not start processing
    assert!(!app.pending_turn);
    assert!(!app.is_processing);
    // Should have an error message about unknown skill
    assert_eq!(app.display_messages().len(), 1);
    assert_eq!(app.display_messages()[0].role, "error");
}

#[test]
fn test_multiple_queued_messages() {
    let mut app = create_test_app();
    app.is_processing = true;

    // Queue first message
    for c in "first".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }
    app.handle_key(KeyCode::Enter, KeyModifiers::CONTROL)
        .unwrap();

    // Queue second message
    for c in "second".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }
    app.handle_key(KeyCode::Enter, KeyModifiers::CONTROL)
        .unwrap();

    // Queue third message
    for c in "third".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }
    app.handle_key(KeyCode::Enter, KeyModifiers::CONTROL)
        .unwrap();

    assert_eq!(app.queued_count(), 3);
    assert_eq!(app.queued_messages()[0], "first");
    assert_eq!(app.queued_messages()[1], "second");
    assert_eq!(app.queued_messages()[2], "third");
    assert!(app.input().is_empty());
}

#[test]
fn test_queue_message_combines_on_send() {
    let mut app = create_test_app();

    // Queue two messages directly
    app.queued_messages.push("message one".to_string());
    app.queued_messages.push("message two".to_string());

    // Take and combine (simulating what process_queued_messages does)
    let combined = std::mem::take(&mut app.queued_messages).join("\n\n");

    assert_eq!(combined, "message one\n\nmessage two");
    assert!(app.queued_messages.is_empty());
}

#[test]
fn test_interleave_message_separate_from_queue() {
    let mut app = create_test_app();
    app.is_processing = true;
    app.queue_mode = false; // Default mode: Enter=interleave, Ctrl+Enter=queue

    // Type and submit via Enter (should interleave, not queue)
    for c in "urgent".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    // Should be in interleave_message, not queued
    assert_eq!(app.interleave_message.as_deref(), Some("urgent"));
    assert_eq!(app.queued_count(), 0);

    // Now queue one
    for c in "later".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }
    app.handle_key(KeyCode::Enter, KeyModifiers::CONTROL)
        .unwrap();

    // Interleave unchanged, one message queued
    assert_eq!(app.interleave_message.as_deref(), Some("urgent"));
    assert_eq!(app.queued_count(), 1);
    assert_eq!(app.queued_messages()[0], "later");
}

#[test]
fn test_handle_paste_single_line() {
    let mut app = create_test_app();

    app.handle_paste("hello world".to_string());

    // Small paste (< 5 lines) is inlined directly
    assert_eq!(app.input(), "hello world");
    assert_eq!(app.cursor_pos(), 11);
    assert!(app.pasted_contents.is_empty()); // No placeholder storage needed
}

#[test]
fn test_terminal_file_drop_submits_as_user_input_instead_of_a_skill() {
    let mut app = create_test_app();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("dropped notes.txt");
    std::fs::write(&file, b"notes").unwrap();
    let dropped = file.display().to_string();

    app.handle_paste(dropped.clone());
    assert_eq!(app.input(), dropped);

    app.submit_input();

    assert!(
        app.is_processing,
        "the dropped file path should start a turn"
    );
    assert!(
        app.display_messages()
            .iter()
            .all(|message| message.role != "error"),
        "a dropped absolute path must not produce an unknown-skill error"
    );
    let submitted = app
        .session
        .messages
        .last()
        .expect("submitted file path message");
    assert!(matches!(
        submitted.content.as_slice(),
        [ContentBlock::Text { text, .. }] if text == &file.display().to_string()
    ));
}

#[test]
fn test_terminal_escaped_file_drop_normalizes_the_path() {
    let mut app = create_test_app();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("dropped notes.txt");
    std::fs::write(&file, b"notes").unwrap();
    let escaped = file.display().to_string().replace(' ', "\\ ");

    app.handle_paste(escaped);

    assert_eq!(app.input(), file.display().to_string());
}

#[test]
fn test_terminal_file_drop_with_followup_text_stays_normal_input() {
    let mut app = create_test_app();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("report.md");
    std::fs::write(&file, b"report").unwrap();
    let prompt = format!("{} please review this file", file.display());
    app.set_input_for_test(prompt.clone());

    app.submit_input();

    assert!(app.is_processing, "the file prompt should start a turn");
    assert!(
        app.display_messages()
            .iter()
            .all(|message| message.role != "error"),
        "a path followed by instructions must not be parsed as a skill"
    );
    let submitted = app
        .session
        .messages
        .last()
        .expect("submitted file prompt message");
    assert!(matches!(
        submitted.content.as_slice(),
        [ContentBlock::Text { text, .. }] if text == &prompt
    ));
}

#[test]
fn test_mixed_file_and_image_drop_keeps_file_and_attaches_image() {
    let mut app = create_test_app();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes with spaces.txt");
    let image = dir.path().join("screenshot.png");
    std::fs::write(&file, b"notes").unwrap();
    std::fs::write(&image, crate::tui::app::input::tiny_png_bytes_for_test()).unwrap();
    let dropped = format!(
        "{} {}",
        file.display().to_string().replace(' ', "\\ "),
        image.display()
    );

    app.handle_paste(dropped);

    assert_eq!(app.input(), format!("\"{}\" [image 1]", file.display()));
    assert_eq!(app.pending_images.len(), 1);
    assert_eq!(app.pending_images[0].0, "image/png");
}

#[test]
fn test_terminal_image_drop_attaches_image_instead_of_routing_as_a_skill() {
    let mut app = create_test_app();
    let dir = tempfile::tempdir().unwrap();
    let image = dir.path().join("dropped screenshot.png");
    std::fs::write(&image, crate::tui::app::input::tiny_png_bytes_for_test()).unwrap();

    app.handle_paste(image.display().to_string());

    assert_eq!(app.input(), "[image 1]");
    assert_eq!(app.pending_images.len(), 1);
    assert_eq!(app.pending_images[0].0, "image/png");
    assert!(
        app.display_messages()
            .iter()
            .all(|message| message.role != "error")
    );
}

#[test]
fn test_typed_absolute_image_path_promotes_before_slash_routing() {
    let mut app = create_test_app();
    let dir = tempfile::tempdir().unwrap();
    let image = dir.path().join("dropped photo.png");
    std::fs::write(&image, crate::tui::app::input::tiny_png_bytes_for_test()).unwrap();
    app.set_input_for_test(image.display().to_string());

    assert!(crate::tui::app::input::promote_dropped_images(&mut app));
    assert_eq!(app.input(), "[image 1]");
    assert_eq!(app.pending_images.len(), 1);
    assert_eq!(app.pending_images[0].0, "image/png");
}

#[test]
fn test_incremental_terminal_drop_promotes_immediately_when_path_completes() {
    let mut app = create_test_app();
    let dir = tempfile::tempdir().unwrap();
    let image = dir.path().join("instant.png");
    std::fs::write(&image, crate::tui::app::input::tiny_png_bytes_for_test()).unwrap();

    for ch in image.display().to_string().chars() {
        crate::tui::app::input::handle_text_input(&mut app, &ch.to_string());
    }

    assert_eq!(app.input(), "[image 1]");
    assert_eq!(app.pending_images.len(), 1);
}

#[test]
fn test_handle_paste_multi_line() {
    let mut app = create_test_app();

    app.handle_paste("line 1\nline 2\nline 3".to_string());

    // Small paste (< 5 lines) is inlined directly
    assert_eq!(app.input(), "line 1\nline 2\nline 3");
    assert!(app.pasted_contents.is_empty());
}

#[test]
fn test_handle_paste_large() {
    let mut app = create_test_app();

    app.handle_paste("a\nb\nc\nd\ne".to_string());

    // Large paste (5+ lines) uses placeholder
    assert_eq!(app.input(), "[pasted 5 lines]");
    assert_eq!(app.pasted_contents.len(), 1);
}

#[test]
fn test_paste_again_expands_placeholder_in_place() {
    let mut app = create_test_app();
    let big = "α\nβ\nγ\nδ\nε".to_string();

    app.handle_paste(big.clone());
    app.handle_key(KeyCode::Char('!'), KeyModifiers::empty())
        .unwrap();
    app.handle_paste(big.clone());

    assert_eq!(app.input(), format!("{big}!"));
    assert_eq!(app.cursor_pos, big.len());
    assert!(app.pasted_contents.is_empty());
}

#[test]
fn test_paste_again_with_different_text_still_collapses() {
    let mut app = create_test_app();

    app.handle_paste("a\nb\nc\nd\ne".to_string());
    app.handle_paste("f\ng\nh\ni\nj".to_string());

    assert_eq!(app.input(), "[pasted 5 lines][pasted 5 lines]");
    assert_eq!(app.pasted_contents.len(), 2);
}

#[test]
fn test_paste_again_expands_matching_placeholder_not_newer_same_sized_paste() {
    let mut app = create_test_app();
    let first = "a\nb\nc\nd\ne".to_string();
    let second = "f\ng\nh\ni\nj".to_string();

    app.handle_paste(first.clone());
    app.handle_key(KeyCode::Char(' '), KeyModifiers::empty())
        .unwrap();
    app.handle_paste(second.clone());
    app.handle_paste(first.clone());

    assert_eq!(app.input(), format!("{first} [pasted 5 lines]"));
    assert_eq!(app.cursor_pos, first.len());
    let visible_input = app.input().to_string();
    assert_eq!(
        crate::tui::app::input::expand_paste_placeholders(&mut app, &visible_input),
        format!("{first} {second}")
    );
    assert_eq!(app.pasted_contents, vec![second]);
}

#[test]
fn test_paste_again_expands_only_most_recent_identical_placeholder() {
    let mut app = create_test_app();
    let big = "a\nb\nc\nd\ne".to_string();

    app.set_input_for_test("[pasted 5 lines] [pasted 5 lines]");
    app.pasted_contents = vec![big.clone(), big.clone()];
    app.handle_paste(big.clone());

    assert_eq!(app.input(), format!("[pasted 5 lines] {big}"));
    assert_eq!(app.pasted_contents, vec![big]);
}

#[test]
fn test_paste_again_does_not_expand_an_edited_placeholder() {
    let mut app = create_test_app();
    let big = "a\nb\nc\nd\ne".to_string();

    app.handle_paste(big.clone());
    app.handle_key(KeyCode::Backspace, KeyModifiers::empty())
        .unwrap();
    app.handle_paste(big);

    assert_eq!(
        app.input(),
        "[pasted 5 lines[pasted 5 lines]",
        "an edited placeholder must not be mistaken for the original"
    );
    assert_eq!(app.pasted_contents.len(), 2);
}

#[test]
fn test_paste_expansion_on_submit() {
    let mut app = create_test_app();

    // Type prefix, paste large content, type suffix
    app.handle_key(KeyCode::Char('A'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char(':'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char(' '), KeyModifiers::empty())
        .unwrap();
    // Paste 5 lines to trigger placeholder
    app.handle_paste("1\n2\n3\n4\n5".to_string());
    app.handle_key(KeyCode::Char(' '), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('B'), KeyModifiers::empty())
        .unwrap();

    // Input shows placeholder
    assert_eq!(app.input(), "A: [pasted 5 lines] B");

    // Submit expands placeholder
    app.submit_input();

    // Sent transcript renders the actual pasted content, while the composer above stayed compact.
    assert_eq!(app.display_messages().len(), 1);
    assert_eq!(app.display_messages()[0].content, "A: 1\n2\n3\n4\n5 B");

    // Model receives expanded content (actual pasted text). Local sessions keep the
    // provider message cache lazy, so inspect the materialized provider view.
    let provider_messages = app.materialized_provider_messages();
    let user_message = provider_messages
        .iter()
        .rev()
        .find(|message| message.role == Role::User)
        .expect("expected submitted user message");
    match &user_message.content[0] {
        crate::message::ContentBlock::Text { text, .. } => {
            assert_eq!(text, "A: 1\n2\n3\n4\n5 B");
        }
        _ => panic!("Expected Text content block"),
    }

    // Pasted contents should be cleared
    assert!(app.pasted_contents.is_empty());
}

#[test]
fn test_multiple_pastes() {
    let mut app = create_test_app();

    // Small pastes are inlined
    app.handle_paste("first".to_string());
    app.handle_key(KeyCode::Char(' '), KeyModifiers::empty())
        .unwrap();
    app.handle_paste("second\nline".to_string());

    // Both small pastes inlined directly
    assert_eq!(app.input(), "first second\nline");
    assert!(app.pasted_contents.is_empty());

    app.submit_input();
    // Display and model both get the same content (no expansion needed)
    assert_eq!(app.display_messages()[0].content, "first second\nline");
    let provider_messages = app.materialized_provider_messages();
    let user_message = provider_messages
        .iter()
        .rev()
        .find(|message| message.role == Role::User)
        .expect("expected submitted user message");
    match &user_message.content[0] {
        crate::message::ContentBlock::Text { text, .. } => {
            assert_eq!(text, "first second\nline");
        }
        _ => panic!("Expected Text content block"),
    }
}

#[test]
fn test_restore_session_adds_reload_message() {
    use crate::session::Session;

    let mut app = create_test_app();

    // Create and save a session with a fake provider_session_id
    let mut session = Session::create(None, None);
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "test message".to_string(),
            cache_control: None,
        }],
    );
    session.provider_session_id = Some("fake-uuid".to_string());
    let session_id = session.id.clone();
    session.save().unwrap();

    // Restore the session
    app.restore_session(&session_id);

    // Should have the original message + reload success message in display
    assert_eq!(app.display_messages().len(), 2);
    assert_eq!(app.display_messages()[0].role, "user");
    assert_eq!(app.display_messages()[0].content, "test message");
    assert_eq!(app.display_messages()[1].role, "system");
    assert!(
        app.display_messages()[1]
            .content
            .contains("Reload complete - continuing.")
    );

    // Local restore keeps provider messages lazy until the next active turn.
    assert_eq!(app.messages.len(), 0);
    assert_eq!(
        app.session.debug_memory_profile()["provider_messages_cache"]["count"],
        0
    );

    // Provider session ID should be cleared (Claude sessions don't persist across restarts)
    assert!(app.provider_session_id.is_none());

    // Clean up
    let _ = std::fs::remove_file(crate::session::session_path(&session_id).unwrap());
}

#[test]
fn test_restore_session_with_selfdev_reload_tool_result_queues_continuation() {
    use crate::session::Session;

    let mut app = create_test_app();

    let mut session = Session::create(None, None);
    session.add_message(
        Role::User,
        vec![ContentBlock::ToolResult {
            tool_use_id: "tool_selfdev_reload".to_string(),
            content: "Reload initiated. Process restarting...".to_string(),
            is_error: Some(false),
        }],
    );
    let session_id = session.id.clone();
    session.save().unwrap();

    app.restore_session(&session_id);

    assert!(
        app.hidden_queued_system_messages
            .iter()
            .any(|message| message.contains("Continue exactly where you left off"))
    );
    assert!(app.pending_turn);
    assert!(matches!(app.status, ProcessingStatus::Sending));

    let _ = std::fs::remove_file(crate::session::session_path(&session_id).unwrap());
}

#[test]
fn test_system_reminder_is_added_to_system_prompt_not_user_messages() {
    let mut app = create_test_app();
    app.current_turn_system_reminder = Some(
        "Your session was interrupted by a server reload. Continue where you left off.".to_string(),
    );

    let split = app.build_system_prompt_split(None);

    assert!(split.dynamic_part.contains("# System Reminder"));
    assert!(split.dynamic_part.contains("Continue where you left off."));
    assert!(app.messages.is_empty());
}

#[test]
fn test_recover_session_without_tools_preserves_debug_and_canary_flags() {
    let mut app = create_test_app();
    app.session.is_debug = true;
    app.session.is_canary = true;
    app.session.testing_build = Some("self-dev".to_string());
    app.session.working_dir = Some("/tmp/jcode-test".to_string());
    let old_session_id = app.session.id.clone();

    app.recover_session_without_tools();

    assert_ne!(app.session.id, old_session_id);
    assert_eq!(
        app.session.parent_id.as_deref(),
        Some(old_session_id.as_str())
    );
    assert!(app.session.is_debug);
    assert!(app.session.is_canary);
    assert_eq!(app.session.testing_build.as_deref(), Some("self-dev"));
    assert_eq!(app.session.working_dir.as_deref(), Some("/tmp/jcode-test"));

    let _ = std::fs::remove_file(crate::session::session_path(&app.session.id).unwrap());
}

#[test]
fn test_has_newer_binary_detection() {
    use std::time::{Duration, SystemTime};

    let mut app = create_test_app();
    let exe = crate::build::launcher_binary_path().unwrap();

    let mut created = false;
    if !exe.exists() {
        if let Some(parent) = exe.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&exe, "test").unwrap();
        created = true;
    }

    app.client_binary_mtime = Some(SystemTime::UNIX_EPOCH);
    assert!(app.has_newer_binary());

    app.client_binary_mtime = Some(SystemTime::now() + Duration::from_secs(3600));
    assert!(!app.has_newer_binary());

    if created {
        let _ = std::fs::remove_file(&exe);
    }
}

#[test]
fn test_reload_requests_exit_when_newer_binary() {
    use std::time::{Duration, SystemTime};

    let mut app = create_test_app();
    let exe = crate::build::launcher_binary_path().unwrap();

    let mut created = false;
    if !exe.exists() {
        if let Some(parent) = exe.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&exe, "test").unwrap();
        created = true;
    }

    app.client_binary_mtime = Some(SystemTime::UNIX_EPOCH);
    app.input = "/reload".to_string();
    app.submit_input();

    assert!(app.reload_requested.is_some());
    assert!(app.should_quit);

    // Ensure the "no newer binary" path is exercised too.
    app.reload_requested = None;
    app.should_quit = false;
    app.client_binary_mtime = Some(SystemTime::now() + Duration::from_secs(3600));
    app.input = "/reload".to_string();
    app.submit_input();
    assert!(app.reload_requested.is_none());
    assert!(!app.should_quit);

    if created {
        let _ = std::fs::remove_file(&exe);
    }
}

#[test]
fn test_background_update_ready_reloads_immediately_when_idle() {
    let mut app = create_test_app();
    let session_id = app.session.id.clone();

    app.handle_session_update_status(SessionUpdateStatus::ReadyToReload {
        session_id: session_id.clone(),
        action: ClientMaintenanceAction::Update,
        version: "v1.2.3".to_string(),
    });

    assert_eq!(app.reload_requested.as_deref(), Some(session_id.as_str()));
    assert!(app.should_quit);
}

#[test]
fn test_background_update_ready_waits_for_turn_to_finish() {
    let mut app = create_test_app();
    let session_id = app.session.id.clone();
    app.is_processing = true;

    app.handle_session_update_status(SessionUpdateStatus::ReadyToReload {
        session_id: session_id.clone(),
        action: ClientMaintenanceAction::Update,
        version: "v1.2.3".to_string(),
    });

    assert!(app.reload_requested.is_none());
    assert_eq!(
        app.pending_background_client_reload
            .as_ref()
            .map(|(id, action)| (id.as_str(), *action)),
        Some((session_id.as_str(), ClientMaintenanceAction::Update))
    );
    assert!(!app.should_quit);

    app.is_processing = false;
    crate::tui::app::local::handle_tick(&mut app);

    assert_eq!(app.reload_requested.as_deref(), Some(session_id.as_str()));
    assert!(app.should_quit);
}

#[test]
fn test_background_update_ready_waits_for_typing_to_go_idle() {
    let mut app = create_test_app();
    let session_id = app.session.id.clone();
    app.note_client_interaction();

    app.handle_session_update_status(SessionUpdateStatus::ReadyToReload {
        session_id: session_id.clone(),
        action: ClientMaintenanceAction::Update,
        version: "v1.2.3".to_string(),
    });

    assert!(app.reload_requested.is_none());
    assert!(!app.should_quit);
    assert_eq!(
        app.status_notice(),
        Some("↑ v1.2.3 ready · reloads when idle".to_string())
    );

    app.last_user_interaction = Some(Instant::now() - Duration::from_secs(2));
    crate::tui::app::local::handle_tick(&mut app);
    assert_eq!(app.reload_requested.as_deref(), Some(session_id.as_str()));
    assert!(app.should_quit);
}

#[test]
fn test_background_rebuild_status_uses_compact_rebuild_card() {
    let mut app = create_test_app();
    let session_id = app.session.id.clone();

    app.handle_session_update_status(SessionUpdateStatus::Status {
        session_id,
        action: ClientMaintenanceAction::Rebuild,
        message: "Building release binary in the background...".to_string(),
    });

    let message = app
        .display_messages()
        .last()
        .expect("expected rebuild display message");
    assert_eq!(message.title.as_deref(), Some("Rebuild"));
    assert!(
        message
            .content
            .contains("Status: Building release binary in the background...")
    );
    assert!(message.content.contains("Pipeline:"));
}

#[test]
fn test_startup_update_checking_stays_quiet_until_update_work_starts() {
    let mut app = create_test_app();

    app.handle_update_status(UpdateStatus::Checking);

    assert!(
        app.display_messages()
            .iter()
            .all(|message| message.title.as_deref() != Some("Update")),
        "startup update checks should not show a card unless an update exists"
    );
    assert_eq!(app.status_notice(), None);

    app.handle_update_status(UpdateStatus::Downloading {
        version: "v1.2.3".to_string(),
        downloaded: 512 * 1024,
        total: Some(1024 * 1024),
    });

    let update_cards = app
        .display_messages()
        .iter()
        .filter(|message| message.title.as_deref() == Some("Update"))
        .count();
    assert_eq!(
        update_cards, 0,
        "background progress should stay out of the transcript"
    );
    let notice = app.status_notice().expect("expected download notice");
    assert!(notice.starts_with("↑ v1.2.3 · Downloading update..."));
    assert!(
        notice.contains("50%"),
        "notice should show progress: {notice}"
    );

    app.handle_update_status(UpdateStatus::Installed {
        version: "v1.2.3".to_string(),
    });

    let message = app
        .display_messages()
        .last()
        .expect("expected update display message");
    assert!(message.content.contains("Status: updated to v1.2.3"));
    assert!(message.content.contains("Restarting now."));
    assert_eq!(
        app.status_notice(),
        Some("Updated to v1.2.3; restarting...".to_string())
    );
}

/// The user-facing complaint behind the progress work: update output used to
/// churn the transcript and clobber the input line. A streaming download must
/// stay in the compact status area, never grow the message list, and never
/// touch the input buffer.
#[test]
fn test_startup_update_progress_stream_does_not_churn_transcript_or_input() {
    let mut app = create_test_app();
    app.set_input_for_test("draft the user was typing".to_string());
    let baseline_messages = app.display_messages().len();

    for downloaded in [0u64, 256, 512, 768, 1024].map(|kib| kib * 1024) {
        app.handle_update_status(UpdateStatus::Downloading {
            version: "v1.2.3".to_string(),
            downloaded,
            total: Some(1024 * 1024),
        });
    }

    assert_eq!(
        app.display_messages().len(),
        baseline_messages,
        "streamed progress must not append transcript cards"
    );
    assert!(
        app.status_notice()
            .is_some_and(|notice| notice.contains("100%")),
        "compact status shows latest progress"
    );
    assert_eq!(
        app.input(),
        "draft the user was typing",
        "update progress must never clobber the input line"
    );
}

#[test]
fn test_startup_update_up_to_date_removes_transient_card() {
    let mut app = create_test_app();

    app.handle_update_status(UpdateStatus::Checking);
    assert!(
        app.display_messages()
            .iter()
            .all(|message| message.title.as_deref() != Some("Update"))
    );

    app.handle_update_status(UpdateStatus::UpToDate);

    assert!(
        app.display_messages()
            .iter()
            .all(|message| message.title.as_deref() != Some("Update")),
        "no-update startup checks should not leave a persistent update card"
    );
    assert!(app.background_client_action.is_none());
    assert!(app.pending_background_client_reload.is_none());
}

#[test]
fn test_startup_update_skipped_stays_quiet() {
    let mut app = create_test_app();
    app.handle_update_status(UpdateStatus::Checking);
    app.handle_update_status(UpdateStatus::Skipped {
        reason: "no upstream configured".to_string(),
    });
    assert!(
        app.display_messages()
            .iter()
            .all(|message| message.title.as_deref() != Some("Update"))
    );
    assert!(app.status_notice().is_none());
    assert!(app.background_client_action.is_none());
    assert!(app.pending_background_client_reload.is_none());
}

#[test]
fn test_startup_update_diverged_offers_merge_without_failure_card() {
    let mut app = create_test_app();

    app.handle_update_status(UpdateStatus::Checking);
    app.handle_update_status(UpdateStatus::Error(
        crate::update::GIT_PULL_DIVERGED_SUMMARY.to_string(),
    ));

    let message = app
        .display_messages()
        .last()
        .expect("expected update display message");
    assert_eq!(message.title.as_deref(), Some("Update"));
    // The diverged card must NOT use the generic failure framing.
    assert!(
        !message.content.contains("Status: failed"),
        "unexpected failure header: {}",
        message.content
    );
    assert!(
        !message
            .content
            .contains("Continuing with the current version."),
        "unexpected continue footer: {}",
        message.content
    );
    // It should explain the divergence and offer the merge-agent hotkey.
    assert!(
        message.content.contains("diverged"),
        "missing divergence explanation: {}",
        message.content
    );
    assert!(
        message.content.to_lowercase().contains("agent"),
        "missing merge-agent hint: {}",
        message.content
    );
    assert!(
        !message.content.contains('\n'),
        "divergence notice should be authored as one line: {}",
        message.content
    );
    assert!(app.pending_merge_offer.is_some());
    assert!(app.background_client_action.is_none());
}

#[test]
fn test_startup_update_diverged_offer_clears_on_submit() {
    let mut app = create_test_app();
    app.handle_update_status(UpdateStatus::Error(format!(
        "Update failed: {}",
        crate::update::GIT_PULL_DIVERGED_SUMMARY
    )));
    assert!(
        app.pending_merge_offer.is_some(),
        "prefixed divergence summary should still arm the offer"
    );

    app.input = "do something else".to_string();
    app.cursor_pos = app.input.len();
    app.submit_input();
    assert!(
        app.pending_merge_offer.is_none(),
        "a fresh submission should drop the stale merge offer"
    );
}

#[test]
fn test_startup_update_error_replaces_checking_card() {
    let mut app = create_test_app();

    app.handle_update_status(UpdateStatus::Checking);
    app.handle_update_status(UpdateStatus::Error("Check failed: offline".to_string()));

    let message = app
        .display_messages()
        .last()
        .expect("expected update display message");
    assert_eq!(message.title.as_deref(), Some("Update"));
    // The failure card and notice are one short line each; the verbose error
    // stays in the log.
    assert_eq!(message.content, "Status: failed (offline)");
    assert!(
        !message.content.contains('\n'),
        "failure card should be one line: {}",
        message.content
    );
    let notice = app.status_notice().expect("expected failure notice");
    assert_eq!(notice, "Update failed: offline");
    assert!(
        !notice.contains('\n'),
        "notice should be one line: {notice}"
    );
    assert!(app.background_client_action.is_none());
    assert!(app.pending_background_client_reload.is_none());
}

#[test]
fn test_selfdev_command_spawns_session_in_test_mode() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    let prev_test = std::env::var_os("JCODE_TEST_SESSION");
    crate::env::set_var("JCODE_HOME", temp_home.path());
    crate::env::set_var("JCODE_TEST_SESSION", "1");

    let repo = create_jcode_repo_fixture();
    let mut app = create_test_app();
    app.session.working_dir = Some(repo.path().display().to_string());

    app.input = "/selfdev fix the markdown renderer".to_string();
    app.submit_input();

    let last = app.display_messages().last().expect("selfdev message");
    assert!(last.content.contains("Created self-dev session"));
    assert!(
        last.content
            .contains("Prompt captured but not delivered in test mode")
    );
    assert_eq!(app.status_notice(), Some("Self-dev".to_string()));

    let sessions_dir = crate::storage::jcode_dir().unwrap().join("sessions");
    let entries: Vec<_> = std::fs::read_dir(&sessions_dir)
        .expect("sessions dir")
        .flatten()
        .collect();
    assert!(
        !entries.is_empty(),
        "expected spawned self-dev session file"
    );

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
    if let Some(prev_test) = prev_test {
        crate::env::set_var("JCODE_TEST_SESSION", prev_test);
    } else {
        crate::env::remove_var("JCODE_TEST_SESSION");
    }
}

#[test]
fn test_save_and_restore_reload_state_preserves_queued_messages() {
    let mut app = create_test_app();
    let session_id = format!("test-reload-{}", std::process::id());

    app.input = "draft".to_string();
    app.cursor_pos = 3;
    app.queued_messages.push("queued one".to_string());
    app.queued_messages.push("queued two".to_string());
    app.hidden_queued_system_messages
        .push("continue silently".to_string());
    app.save_input_for_reload(&session_id);

    let restored = App::restore_input_for_reload(&session_id).expect("reload state should exist");
    assert_eq!(restored.input, "draft");
    assert_eq!(restored.cursor, 3);
    assert_eq!(restored.queued_messages, vec!["queued one", "queued two"]);
    assert_eq!(
        restored.hidden_queued_system_messages,
        vec!["continue silently"]
    );

    assert!(App::restore_input_for_reload(&session_id).is_none());
}

#[test]
fn test_new_for_remote_restored_queued_messages_stay_queued_until_remote_idle() {
    let mut app = create_test_app();
    let session_id = format!("test-remote-queued-restore-{}", std::process::id());

    app.queued_messages.push("queued one".to_string());
    app.queued_messages.push("queued two".to_string());
    app.hidden_queued_system_messages
        .push("continue silently".to_string());
    app.save_input_for_reload(&session_id);

    let restored = App::new_for_remote(Some(session_id));
    assert_eq!(restored.queued_messages(), &["queued one", "queued two"]);
    assert_eq!(
        restored.hidden_queued_system_messages,
        vec!["continue silently"]
    );
    assert!(!restored.pending_queued_dispatch);
    assert!(!restored.is_processing);
    assert!(matches!(restored.status, ProcessingStatus::Idle));
}

#[test]
fn test_save_and_restore_startup_submission_preserves_pending_images() {
    with_temp_jcode_home(|| {
        let session_id = "session_startup_prompt";
        App::save_startup_submission_for_session(
            session_id,
            "describe this".to_string(),
            vec![("image/png".to_string(), "abc123".to_string())],
        );

        let restored =
            App::restore_input_for_reload(session_id).expect("startup submission should restore");
        assert_eq!(restored.input, "describe this");
        assert!(restored.submit_on_restore);
        assert_eq!(restored.pending_images.len(), 1);
        assert_eq!(restored.pending_images[0].0, "image/png");
        assert_eq!(restored.pending_images[0].1, "abc123");
    });
}

#[test]
fn test_save_and_restore_reload_state_preserves_interleave_and_pending_retry() {
    let mut app = create_test_app();
    let session_id = format!("test-reload-pending-{}", std::process::id());

    app.input = "draft".to_string();
    app.cursor_pos = 5;
    app.interleave_message = Some("urgent now".to_string());
    app.pending_soft_interrupts = vec![
        "already sent one".to_string(),
        "already sent two".to_string(),
    ];
    app.pending_soft_interrupt_requests = vec![(17, "already sent two".to_string())];
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "retry me".to_string(),
        images: vec![("image/png".to_string(), "abc123".to_string())],
        is_system: true,
        system_reminder: Some("continue silently".to_string()),
        auto_retry: true,
        retry_attempts: 2,
        retry_at: None,
    });
    app.rate_limit_reset = Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
    app.save_input_for_reload(&session_id);

    let restored = App::restore_input_for_reload(&session_id).expect("reload state should exist");
    assert_eq!(restored.interleave_message.as_deref(), Some("urgent now"));
    assert_eq!(
        restored.pending_soft_interrupts,
        vec!["already sent one", "already sent two"]
    );
    assert_eq!(
        restored.pending_soft_interrupt_resend,
        Some(vec!["already sent two".to_string()])
    );

    let pending = restored
        .rate_limit_pending_message
        .expect("pending retry should restore");
    assert_eq!(pending.content, "retry me");
    assert_eq!(
        pending.images,
        vec![("image/png".to_string(), "abc123".to_string())]
    );
    assert!(pending.is_system);
    assert_eq!(
        pending.system_reminder.as_deref(),
        Some("continue silently")
    );
    assert!(pending.auto_retry);
    assert_eq!(pending.retry_attempts, 2);
    assert!(pending.retry_at.is_some());
    assert!(restored.rate_limit_reset.is_some());
}

#[test]
fn test_save_and_restore_reload_state_promotes_inflight_prompt_to_startup_submission() {
    let mut app = create_test_app();
    let session_id = format!("test-reload-inflight-prompt-{}", std::process::id());

    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "finish the refactor".to_string(),
        images: vec![("image/png".to_string(), "abc123".to_string())],
        is_system: false,
        system_reminder: None,
        auto_retry: false,
        retry_attempts: 0,
        retry_at: None,
    });
    app.rate_limit_reset = Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
    app.save_input_for_reload(&session_id);

    let restored = App::restore_input_for_reload(&session_id).expect("reload state should exist");
    assert_eq!(restored.input, "finish the refactor");
    assert_eq!(restored.cursor, "finish the refactor".len());
    assert!(
        restored.submit_on_restore,
        "in-flight prompt should resume automatically"
    );
    assert_eq!(restored.pending_images.len(), 1);
    assert!(
        restored.rate_limit_pending_message.is_none(),
        "promoted startup submission should not linger as a passive pending retry"
    );
}

#[test]
fn test_save_and_restore_reload_state_preserves_observe_mode() {
    let mut app = create_test_app();
    let session_id = format!("test-reload-observe-{}", std::process::id());

    app.set_observe_mode_enabled(true, true);
    app.observe_page_markdown = "# Observe\n\nPersist me through reload.".to_string();
    app.observe_page_updated_at_ms = 42;
    app.save_input_for_reload(&session_id);

    let restored = App::restore_input_for_reload(&session_id).expect("reload state should exist");
    assert!(restored.observe_mode_enabled);
    assert_eq!(
        restored.observe_page_markdown,
        "# Observe\n\nPersist me through reload."
    );
    assert_eq!(restored.observe_page_updated_at_ms, 42);
}

#[test]
fn test_save_and_restore_reload_state_preserves_split_view_mode() {
    let mut app = create_test_app();
    let session_id = format!("test-reload-splitview-{}", std::process::id());

    app.set_split_view_enabled(true, true);
    app.save_input_for_reload(&session_id);

    let restored = App::restore_input_for_reload(&session_id).expect("reload state should exist");
    assert!(restored.split_view_enabled);
}

#[test]
fn test_new_for_remote_restores_observe_mode_from_reload_state() {
    let mut app = create_test_app();
    let session_id = format!("test-remote-observe-{}", std::process::id());

    app.set_observe_mode_enabled(true, true);
    app.observe_page_markdown = "# Observe\n\nRestored after reload.".to_string();
    app.observe_page_updated_at_ms = 99;
    app.save_input_for_reload(&session_id);

    let restored = App::new_for_remote(Some(session_id));
    assert!(restored.observe_mode_enabled());
    let page = restored
        .side_panel()
        .focused_page()
        .expect("observe page should be focused");
    assert_eq!(page.id, "observe");
    assert!(page.content.contains("Restored after reload."));
}

#[test]
fn test_new_for_remote_restores_split_view_from_reload_state() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        let session_id = "test-remote-splitview";

        app.set_split_view_enabled(true, true);
        app.save_input_for_reload(session_id);

        let restored = App::new_for_remote(Some(session_id.to_string()));
        assert!(restored.split_view_enabled());
        let page = restored
            .side_panel()
            .focused_page()
            .expect("split view page should be focused");
        assert_eq!(page.id, "split_view");
        assert!(page.content.contains("Split View"));
    });
}

#[test]
fn test_restore_reload_state_supports_legacy_input_format() {
    let session_id = format!("test-reload-legacy-{}", std::process::id());
    let jcode_dir = crate::storage::jcode_dir().unwrap();
    let path = jcode_dir.join(format!("client-input-{}", session_id));
    std::fs::write(&path, "2\nhello").unwrap();

    let restored =
        App::restore_input_for_reload(&session_id).expect("legacy reload state should restore");
    assert_eq!(restored.input, "hello");
    assert_eq!(restored.cursor, 2);
    assert!(restored.queued_messages.is_empty());
}

#[test]
fn test_new_for_remote_requeues_restored_pending_soft_interrupts() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        let session_id = "test-remote-restore";

        app.interleave_message = Some("local interleave".to_string());
        app.pending_soft_interrupts = vec!["sent one".to_string(), "sent two".to_string()];
        app.pending_soft_interrupt_requests =
            vec![(101, "sent one".to_string()), (102, "sent two".to_string())];
        app.queued_messages.push("queued later".to_string());
        app.save_input_for_reload(session_id);

        let restored = App::new_for_remote(Some(session_id.to_string()));
        assert!(restored.interleave_message.is_none());
        assert_eq!(
            restored.queued_messages(),
            &["local interleave", "sent one", "sent two", "queued later"]
        );
    });
}

#[test]
fn test_new_for_remote_restored_interleave_triggers_dispatch_state() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        let session_id = "test-remote-interleave-dispatch";

        app.interleave_message = Some("interrupt after reload".to_string());
        app.save_input_for_reload(session_id);

        let mut restored = App::new_for_remote(Some(session_id.to_string()));
        assert!(restored.interleave_message.is_none());
        assert_eq!(restored.queued_messages(), &["interrupt after reload"]);
        assert!(!restored.pending_queued_dispatch);
        assert!(!restored.is_processing);
        assert!(matches!(restored.status, ProcessingStatus::Idle));

        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        rt.block_on(super::remote::process_remote_followups(
            &mut restored,
            &mut remote,
        ));
        assert_eq!(restored.queued_messages(), &["interrupt after reload"]);
        assert!(!restored.is_processing);

        remote.mark_history_loaded();
        rt.block_on(super::remote::process_remote_followups(
            &mut restored,
            &mut remote,
        ));

        assert!(restored.queued_messages().is_empty());
        assert!(restored.is_processing);
        assert!(matches!(restored.status, ProcessingStatus::Sending));
        assert!(restored.display_messages().iter().any(|message| {
            message.role == "user" && message.content == "interrupt after reload"
        }));
    });
}

#[test]
fn test_passive_restore_preserves_followups_across_ticks_and_reopen() {
    with_temp_jcode_home(|| {
        let id = "session_passive_queues";
        let mut original = create_test_app();
        original.input = "draft".into();
        original.cursor_pos = 3;
        original.queued_messages.push("queued".into());
        original.hidden_queued_system_messages.push("hidden".into());
        original.interleave_message = Some("interrupt".into());
        original.pending_soft_interrupts.push("pending".into());
        original
            .pending_soft_interrupt_requests
            .push((17, "pending".into()));
        original.rate_limit_pending_message = Some(PendingRemoteMessage {
            content: "retry".into(),
            images: vec![],
            is_system: false,
            system_reminder: None,
            auto_retry: true,
            retry_attempts: 1,
            retry_at: None,
        });
        original.rate_limit_reset = Some(std::time::Instant::now());
        original.save_input_for_reload(id);
        crate::restart_snapshot::mark_passive_restore(id).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        for _ in 0..2 {
            let mut restored = App::new_for_remote(Some(id.into()));
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            remote.mark_history_loaded();
            assert!(restored.passive_restart_restore);
            rt.block_on(super::remote::process_remote_followups(
                &mut restored,
                &mut remote,
            ));
            rt.block_on(super::remote::handle_tick(&mut restored, &mut remote));
            assert_eq!(restored.input, "draft");
            assert_eq!(restored.cursor_pos, 3);
            assert_eq!(
                restored.queued_messages(),
                &["interrupt", "pending", "queued"]
            );
            assert_eq!(restored.hidden_queued_system_messages, vec!["hidden"]);
            assert_eq!(
                restored
                    .rate_limit_pending_message
                    .as_ref()
                    .unwrap()
                    .content,
                "retry"
            );
            assert!(!restored.is_processing);
            assert!(restored.current_message_id.is_none());
        }
        assert!(App::restore_input_for_reload(id).is_some());
    });
}

#[test]
fn test_passive_restore_accepts_fresh_prompt_after_history_loads() {
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut app = create_test_app();
        app.passive_restart_restore = true;
        app.input = "new instructions".into();
        let prepared = super::input::take_prepared_input(&mut app);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        rt.block_on(super::remote::submit_prepared_remote_input(
            &mut app,
            &mut remote,
            prepared,
        ))
        .unwrap();
        rt.block_on(super::remote::process_remote_followups(
            &mut app,
            &mut remote,
        ));
        assert!(app.passive_restart_restore);
        assert!(!app.is_processing);
        remote.mark_history_loaded();
        rt.block_on(super::remote::process_remote_followups(
            &mut app,
            &mut remote,
        ));
        assert!(!app.passive_restart_restore);
        assert!(app.is_processing);
        assert!(app.pending_prompt_before_history.is_none());
        assert!(app.current_message_id.is_some());
    });
}

#[test]
fn test_passive_restore_followups_survive_fresh_prompt_and_repeated_reopen() {
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let id = "session_durable_followups";
        let mut original = create_test_app();
        original.queued_messages.push("saved follow-up".into());
        original
            .hidden_queued_system_messages
            .push("saved hidden follow-up".into());
        original.save_input_for_reload(id);
        crate::restart_snapshot::mark_passive_restore(id).unwrap();
        let mut app = App::new_for_remote(Some(id.into()));
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.set_session_id(id.into());
        remote.mark_history_loaded();
        app.input = "fresh prompt".into();
        let prepared = super::input::take_prepared_input(&mut app);
        rt.block_on(super::remote::submit_prepared_remote_input(
            &mut app,
            &mut remote,
            prepared,
        ))
        .unwrap();
        crate::restart_snapshot::clear_passive_restore(id).unwrap();
        // Simulate crashes without graceful-save hooks, including a crash
        // after reopening but before the saved queue can dispatch.
        drop(app);
        for _ in 0..2 {
            let reopened = App::new_for_remote(Some(id.into()));
            assert_eq!(reopened.queued_messages(), &["saved follow-up"]);
            assert_eq!(
                reopened.hidden_queued_system_messages,
                vec!["saved hidden follow-up"]
            );
            assert!(!reopened.submit_input_on_startup);
            reopened.save_input_for_reload(id);
        }
        let mut reopened = App::new_for_remote(Some(id.into()));
        rt.block_on(super::remote::process_remote_followups(
            &mut reopened,
            &mut remote,
        ));
        assert!(reopened.is_processing);
        assert!(reopened.queued_messages().is_empty());
        assert!(reopened.hidden_queued_system_messages.is_empty());
        let unread = App::new_for_remote(Some(id.into()));
        assert_eq!(unread.restored_retries.len(), 1);
        assert_eq!(unread.restored_retries[0].content, "saved follow-up");
        assert_eq!(
            unread.restored_retries[0].system_reminder.as_deref(),
            Some("saved hidden follow-up")
        );
        let request_id = reopened.current_message_id.unwrap();
        reopened.handle_server_event(
            crate::protocol::ServerEvent::Done { id: request_id },
            &mut remote,
        );
        let after_dispatch = App::new_for_remote(Some(id.into()));
        assert!(
            after_dispatch.queued_messages().is_empty(),
            "sent follow-ups must not replay"
        );
        assert!(after_dispatch.hidden_queued_system_messages.is_empty());
        assert!(after_dispatch.restored_retries.is_empty());
    });
}

#[test]
fn test_passive_restore_checkpoints_legacy_plain_text_input() {
    with_temp_jcode_home(|| {
        let id = "session_legacy_plain_input";
        let path = crate::storage::jcode_dir()
            .unwrap()
            .join(format!("client-input-{id}"));
        std::fs::write(&path, "legacy draft").unwrap();
        let mut app = create_test_app();
        app.passive_restart_restore = true;
        app.queued_messages.push("remaining follow-up".into());
        app.checkpoint_restored_followups(id).unwrap();
        let restored = App::restore_input_for_reload(id).unwrap();
        assert_eq!(restored.queued_messages, vec!["remaining follow-up"]);
        assert!(path.exists());
    });
}

#[test]
fn test_passive_restore_retry_survives_fresh_prompt_crash_and_dispatch() {
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        for is_system in [false, true] {
            let id = if is_system {
                "session_saved_system_retry"
            } else {
                "session_saved_user_retry"
            };
            let path = crate::storage::jcode_dir()
                .unwrap()
                .join(format!("client-input-{id}"));
            let mut original = create_test_app();
            original.rate_limit_pending_message = Some(PendingRemoteMessage {
                content: "saved retry".into(),
                images: vec![("image/png".into(), "payload".into())],
                is_system,
                system_reminder: Some("saved reminder".into()),
                auto_retry: true,
                retry_attempts: 2,
                retry_at: None,
            });
            original.rate_limit_reset =
                Some(std::time::Instant::now() + std::time::Duration::from_secs(60));
            original.save_input_for_reload(id);
            crate::restart_snapshot::mark_passive_restore(id).unwrap();
            let mut app = App::new_for_remote(Some(id.into()));
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            remote.set_session_id(id.into());
            remote.mark_history_loaded();
            app.input = "fresh prompt".into();
            let prepared = super::input::take_prepared_input(&mut app);
            rt.block_on(super::remote::submit_prepared_remote_input(
                &mut app,
                &mut remote,
                prepared,
            ))
            .unwrap();
            crate::restart_snapshot::clear_passive_restore(id).unwrap();
            let mut saved: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(saved["restored_retries"][0]["content"], "saved retry");
            assert_eq!(saved["restored_retries"][0]["images"][0][1], "payload");
            assert!(
                saved["restored_retries"][0]["retry_deadline_unix_ms"]
                    .as_u64()
                    .unwrap()
                    > 0
            );
            drop(app);
            for _ in 0..2 {
                let mut reopened = App::new_for_remote(Some(id.into()));
                rt.block_on(super::remote::process_remote_followups(
                    &mut reopened,
                    &mut remote,
                ));
                assert!(!reopened.is_processing, "retry must respect its delay");
                reopened.save_input_for_reload(id);
            }
            saved = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            saved["restored_retries"][0]["retry_deadline_unix_ms"] = serde_json::json!(0);
            std::fs::write(&path, saved.to_string()).unwrap();
            let mut reopened = App::new_for_remote(Some(id.into()));
            reopened.rate_limit_reset =
                Some(std::time::Instant::now() + std::time::Duration::from_secs(60));
            rt.block_on(super::remote::process_remote_followups(
                &mut reopened,
                &mut remote,
            ));
            assert!(
                reopened.current_message_id.is_none(),
                "preserve the fresh prompt's own retry window"
            );
            reopened.rate_limit_reset = None;
            let mut disconnected = crate::tui::backend::RemoteConnection::dummy();
            disconnected.set_session_id(id.into());
            disconnected.mark_history_loaded();
            drop(disconnected.take_dummy_peer());
            rt.block_on(super::remote::process_remote_followups(
                &mut reopened,
                &mut disconnected,
            ));
            assert!(!reopened.is_processing);
            let after_failure: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(
                after_failure["restored_retries"][0]["content"],
                "saved retry"
            );
            reopened.is_processing = true;
            rt.block_on(super::remote::process_remote_followups(
                &mut reopened,
                &mut remote,
            ));
            assert!(
                reopened.current_message_id.is_none(),
                "do not interrupt a running fresh prompt"
            );
            reopened.is_processing = false;
            if is_system {
                rt.block_on(super::remote::handle_tick(&mut reopened, &mut remote));
            } else {
                rt.block_on(super::remote::process_remote_followups(
                    &mut reopened,
                    &mut remote,
                ));
            }
            let sent = reopened.rate_limit_pending_message.as_ref().unwrap();
            assert_eq!(sent.content, "saved retry");
            assert_eq!(sent.images, vec![("image/png".into(), "payload".into())]);
            assert_eq!(sent.system_reminder.as_deref(), Some("saved reminder"));
            assert_eq!(sent.is_system, is_system);
            assert!(sent.auto_retry);
            assert_eq!(sent.retry_attempts, 2);
            // The dummy socket is connected but unread: writing to it is
            // not evidence that the server accepted the request.
            let unread = App::new_for_remote(Some(id.into()));
            assert_eq!(unread.restored_retries.len(), 1);
            reopened.save_input_for_reload(id);
            let saved_inflight = App::restore_input_for_reload(id).unwrap();
            assert_eq!(saved_inflight.restored_retries.len(), 1);
            assert!(saved_inflight.queued_messages.is_empty());
            assert!(saved_inflight.hidden_queued_system_messages.is_empty());
            assert!(saved_inflight.rate_limit_pending_message.is_none());
            let request_id = reopened.current_message_id.unwrap();
            reopened.handle_server_event(
                crate::protocol::ServerEvent::Done {
                    id: request_id + 100,
                },
                &mut remote,
            );
            assert_eq!(
                reopened.restored_retries.len(),
                1,
                "unrelated completion cannot consume a retry"
            );
            reopened.handle_server_event(
                crate::protocol::ServerEvent::Done { id: request_id },
                &mut remote,
            );
            let mut after_dispatch = App::new_for_remote(Some(id.into()));
            rt.block_on(super::remote::process_remote_followups(
                &mut after_dispatch,
                &mut remote,
            ));
            assert!(
                !after_dispatch.is_processing,
                "completed retry must not replay"
            );
        }
    });
}

#[test]
fn test_restored_retry_rejection_retains_checkpoint() {
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let id = "session_rejected_retry";
        let mut app = create_test_app();
        app.restored_retries.push(PendingRemoteMessage {
            content: "keep until completed".into(),
            images: vec![],
            is_system: true,
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
        app.handle_server_event(
            crate::protocol::ServerEvent::Error {
                id: request_id,
                message: "Session is busy".into(),
                retry_after_secs: None,
            },
            &mut remote,
        );
        app.handle_server_event(
            crate::protocol::ServerEvent::Done { id: request_id },
            &mut remote,
        );
        let reopened = App::new_for_remote(Some(id.into()));
        assert_eq!(reopened.restored_retries.len(), 1);
        assert_eq!(reopened.restored_retries[0].content, "keep until completed");
    });
}

#[test]
fn test_restored_retry_deadline_does_not_restart_on_reopen() {
    with_temp_jcode_home(|| {
        for legacy_format in [false, true] {
            let id = "session_retry_deadline";
            let mut app = create_test_app();
            app.restored_retries.push(PendingRemoteMessage {
                content: "due soon".into(),
                images: vec![],
                is_system: true,
                system_reminder: None,
                auto_retry: true,
                retry_attempts: 0,
                retry_at: Some(std::time::Instant::now() + std::time::Duration::from_millis(300)),
            });
            app.save_input_for_reload(id);
            if legacy_format {
                let path = crate::storage::jcode_dir()
                    .unwrap()
                    .join(format!("client-input-{id}"));
                let mut saved: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                saved["restored_retries"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("retry_deadline_unix_ms");
                saved["restored_retries"][0]["retry_delay_ms"] = serde_json::json!(300);
                std::fs::write(path, saved.to_string()).unwrap();
            }
            let before = App::new_for_remote(Some(id.into()));
            assert!(before.restored_retries[0].retry_at.unwrap() > std::time::Instant::now());
            std::thread::sleep(std::time::Duration::from_millis(400));
            let rt = tokio::runtime::Runtime::new().unwrap();
            let _guard = rt.enter();
            let mut after = App::new_for_remote(Some(id.into()));
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            remote.set_session_id(id.into());
            remote.mark_history_loaded();
            rt.block_on(super::remote::process_remote_followups(
                &mut after,
                &mut remote,
            ));
            assert!(
                after.is_processing,
                "the original deadline passed while the window was closed"
            );
        }
    });
}
