#[test]
fn test_build_turn_footer_combines_compact_duration_with_streaming_stats() {
    let mut app = create_test_app();
    app.streaming_input_tokens = 210_000;
    app.streaming_output_tokens = 440;
    app.streaming_tps_collect_output = true;
    app.streaming_total_output_tokens = 440;
    app.streaming_tps_observed_output_tokens = 440;
    app.streaming_tps_observed_elapsed = Duration::from_secs(220);

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

    // Type a skill command
    app.handle_key(KeyCode::Char('/'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('t'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('e'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('s'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('t'), KeyModifiers::empty())
        .unwrap();

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

    // Display shows placeholder (user sees condensed view)
    assert_eq!(app.display_messages().len(), 1);
    assert_eq!(app.display_messages()[0].content, "A: [pasted 5 lines] B");

    // Model receives expanded content (actual pasted text)
    assert_eq!(app.messages.len(), 1);
    match &app.messages[0].content[0] {
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
    match &app.messages[0].content[0] {
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
            .contains("Reload complete — continuing.")
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
            .contains("**Status:** Building release binary in the background...")
    );
    assert!(message.content.contains("**Pipeline:**"));
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
fn test_new_for_remote_restored_queued_messages_triggers_dispatch_state() {
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
    assert!(restored.pending_queued_dispatch);
    assert!(restored.is_processing);
    assert!(matches!(restored.status, ProcessingStatus::Sending));
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
    let mut app = create_test_app();
    let session_id = format!("test-remote-splitview-{}", std::process::id());

    app.set_split_view_enabled(true, true);
    app.save_input_for_reload(&session_id);

    let restored = App::new_for_remote(Some(session_id));
    assert!(restored.split_view_enabled());
    let page = restored
        .side_panel()
        .focused_page()
        .expect("split view page should be focused");
    assert_eq!(page.id, "split_view");
    assert!(page.content.contains("Split View"));
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
    let mut app = create_test_app();
    let session_id = format!("test-remote-restore-{}", std::process::id());

    app.interleave_message = Some("local interleave".to_string());
    app.pending_soft_interrupts = vec!["sent one".to_string(), "sent two".to_string()];
    app.pending_soft_interrupt_requests =
        vec![(101, "sent one".to_string()), (102, "sent two".to_string())];
    app.queued_messages.push("queued later".to_string());
    app.save_input_for_reload(&session_id);

    let restored = App::new_for_remote(Some(session_id));
    assert!(restored.interleave_message.is_none());
    assert_eq!(
        restored.queued_messages(),
        &["local interleave", "sent one", "sent two", "queued later"]
    );
}

#[test]
fn test_new_for_remote_restored_interleave_triggers_dispatch_state() {
    let mut app = create_test_app();
    let session_id = format!("test-remote-interleave-dispatch-{}", std::process::id());

    app.interleave_message = Some("interrupt after reload".to_string());
    app.save_input_for_reload(&session_id);

    let restored = App::new_for_remote(Some(session_id));
    assert!(restored.interleave_message.is_none());
    assert_eq!(restored.queued_messages(), &["interrupt after reload"]);
    assert!(restored.pending_queued_dispatch);
    assert!(restored.is_processing);
    assert!(matches!(restored.status, ProcessingStatus::Sending));
}
