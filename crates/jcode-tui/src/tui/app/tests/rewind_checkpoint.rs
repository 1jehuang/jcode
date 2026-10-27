use super::*;

#[test]
fn test_rewind_selector_lists_user_checkpoints_not_initial_hidden_context() {
    let mut app = create_test_app();

    for idx in 1..=2 {
        let prompt = format!("msg-{}", idx);
        let checkpoint = app.session.create_rewind_checkpoint(&prompt).unwrap();
        app.session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: prompt,
                cache_control: None,
            }],
        );
        app.bind_local_rewind_checkpoint(Some(checkpoint.id));
    }

    app.input = "/rewind".to_string();
    app.submit_input();

    let dialog = app.dialog.as_ref().expect("checkpoint dialog");
    assert!(dialog.is_rewind());
    let rows = dialog.item_summaries();
    assert_eq!(rows.len(), 3);
    assert!(rows[0].starts_with("msg-1"));
    assert!(rows[1].starts_with("msg-2"));
    assert_eq!(rows[2], "(current)");
    assert!(!rows.iter().any(|row| row.contains("Session Context")));
}

#[test]
fn test_rewind_autocomplete_does_not_fuzzy_rewrite_numeric_targets() {
    let mut app = create_test_app();
    app.session.replace_messages(Vec::new());

    for idx in 1..=3 {
        app.session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: format!("msg-{}", idx),
                cache_control: None,
            }],
        );
    }

    app.input = "/rewind 10".to_string();
    assert!(!app.autocomplete());
    assert_eq!(app.input, "/rewind 10");

    app.input = "/rewind 2".to_string();
    assert!(!app.autocomplete());
    assert_eq!(app.input, "/rewind 2");
}
use crate::protocol::{RewindCheckpoint, RewindMode, RewindResult, RewindTargetKind, ServerEvent};
use crate::tui::dialog::Dialog;

fn rewind_result(mode: RewindMode) -> RewindResult {
    RewindResult {
        target_kind: RewindTargetKind::UserPrompt,
        mode,
        prompt: "original 日本語\nsecond line".to_string(),
        images: Vec::new(),
        removed_messages: 3,
        restored_files: 1,
        skipped_files: Vec::new(),
    }
}

fn rewind_checkpoint() -> RewindCheckpoint {
    RewindCheckpoint {
        target_kind: RewindTargetKind::UserPrompt,
        id: "checkpoint-stable".to_string(),
        message_index: 0,
        prompt: "original 日本語\nsecond line".to_string(),
        timestamp: None,
        can_restore_files: true,
        changed_file_count: 1,
    }
}

#[test]
fn checkpoint_rewind_double_escape_is_idle_empty_only_and_resets_on_other_keys() {
    let mut app = create_test_app();
    app.handle_key(KeyCode::Esc, KeyModifiers::empty()).unwrap();
    assert!(app.dialog.is_none());
    assert!(app.rewind_ui.last_empty_escape.is_some());
    app.handle_key(KeyCode::Left, KeyModifiers::empty())
        .unwrap();
    assert!(app.rewind_ui.last_empty_escape.is_none());
    app.handle_key(KeyCode::Esc, KeyModifiers::empty()).unwrap();
    app.handle_key(KeyCode::Esc, KeyModifiers::empty()).unwrap();
    assert!(app.dialog.as_ref().is_some_and(|dialog| dialog.is_rewind()));
    app.handle_key(KeyCode::Esc, KeyModifiers::empty()).unwrap();
    assert!(app.dialog.is_none());
    assert!(app.rewind_ui.last_empty_escape.is_none());

    app.input = "keep my draft".to_string();
    app.cursor_pos = app.input.len();
    app.handle_key(KeyCode::Esc, KeyModifiers::empty()).unwrap();
    assert!(app.input.is_empty());
    assert!(
        app.rewind_ui.last_empty_escape.is_none(),
        "draft clearing cannot arm rewind"
    );
    app.undo_input_change();
    assert_eq!(app.input, "keep my draft");
    app.input.clear();
    app.pending_images
        .push(("image/png".to_string(), "data".to_string()));
    app.handle_key(KeyCode::Esc, KeyModifiers::empty()).unwrap();
    assert!(
        app.rewind_ui.last_empty_escape.is_none(),
        "image-only draft cannot arm rewind"
    );
    app.is_processing = true;
    app.handle_key(KeyCode::Esc, KeyModifiers::empty()).unwrap();
    assert!(app.cancel_requested);
    assert!(app.dialog.is_none());
    assert!(app.rewind_ui.last_empty_escape.is_none());
}

#[test]
fn checkpoint_rewind_escape_timeout_repeat_paste_and_modal_are_not_second_escape() {
    let mut app = create_test_app();
    app.rewind_ui.last_empty_escape = Some(Instant::now() - Duration::from_secs(2));
    app.handle_key(KeyCode::Esc, KeyModifiers::empty()).unwrap();
    assert!(app.dialog.is_none());
    let repeat = crossterm::event::KeyEvent::new_with_kind(
        KeyCode::Esc,
        KeyModifiers::empty(),
        crossterm::event::KeyEventKind::Repeat,
    );
    app.handle_key_press_event(repeat).unwrap();
    assert!(app.dialog.is_none());
    app.handle_paste("pasted".to_string());
    assert!(app.rewind_ui.last_empty_escape.is_none());
    app.input.clear();
    app.rewind_ui.last_empty_escape = Some(Instant::now());
    app.dialog = Some(Dialog::rewind_loading());
    app.handle_key(KeyCode::Esc, KeyModifiers::empty()).unwrap();
    assert!(app.dialog.is_none());
    assert!(app.rewind_ui.last_empty_escape.is_none());
    app.handle_key(KeyCode::Esc, KeyModifiers::empty()).unwrap();
    assert!(
        app.dialog.is_none(),
        "Esc closing an overlay cannot arm rewind"
    );
}

#[test]
fn checkpoint_rewind_completion_restores_exact_prompt_without_submission_and_files_preserve_draft()
{
    let mut app = create_test_app();
    app.input = "existing draft".to_string();
    app.cursor_pos = app.input.len();
    app.dialog = Some(Dialog::rewind_loading());
    app.finish_checkpoint_rewind(rewind_result(RewindMode::FilesOnly));
    assert_eq!(app.input, "existing draft");
    assert!(app.dialog.is_none());
    app.rate_limit_reset = Some(Instant::now());
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "removed pending prompt".to_string(),
        images: Vec::new(),
        is_system: false,
        system_reminder: None,
        auto_retry: true,
        retry_attempts: 0,
        retry_at: Some(Instant::now()),
        overload_attempts: 0,
    });
    app.pending_turn = true;
    app.pending_queued_dispatch = true;
    app.last_submitted_input = Some("old prompt".to_string());
    app.finish_checkpoint_rewind(rewind_result(RewindMode::ConversationOnly));
    assert_eq!(app.input, "original 日本語\nsecond line");
    assert_eq!(app.cursor_pos, app.input.len());
    assert!(!app.pending_turn);
    assert!(!app.pending_queued_dispatch);
    assert!(app.rate_limit_reset.is_none());
    assert!(app.rate_limit_pending_message.is_none());
    assert!(app.last_submitted_input.is_none());
    assert!(!app.submit_input_on_startup);
    app.undo_input_change();
    assert_eq!(app.input, "existing draft");
}

#[test]
fn checkpoint_rewind_remote_command_requests_list_and_correlates_reply() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();
    app.remote_session_id = Some("session-rewind".to_string());
    app.is_remote = true;
    app.input = "/rewind".to_string();
    rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
        .unwrap();
    let id = app.rewind_ui.list_request.as_ref().unwrap().0;
    assert!(app.dialog.as_ref().is_some_and(|dialog| dialog.is_rewind()));
    app.handle_server_event(
        ServerEvent::RewindCheckpoints {
            id: id + 1,
            session_id: "session-rewind".to_string(),
            checkpoints: vec![rewind_checkpoint()],
        },
        &mut remote,
    );
    assert!(app.dialog.as_ref().unwrap().item_summaries().is_empty());
    app.handle_server_event(
        ServerEvent::RewindCheckpoints {
            id,
            session_id: "session-rewind".to_string(),
            checkpoints: vec![rewind_checkpoint()],
        },
        &mut remote,
    );
    assert_eq!(
        app.dialog.as_ref().unwrap().item_summaries(),
        vec!["original 日本語 · You", "(current)"]
    );
    rt.block_on(app.handle_remote_key(KeyCode::Up, KeyModifiers::empty(), &mut remote))
        .unwrap();
    rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
        .unwrap();
    rt.block_on(app.handle_remote_key(KeyCode::Char('2'), KeyModifiers::empty(), &mut remote))
        .unwrap();
    let restore_id = app.rewind_ui.restore_request.as_ref().unwrap().0;
    assert!(!remote.has_loaded_history());
    app.handle_server_event(
        ServerEvent::Error {
            id: restore_id,
            message: "file conflict".to_string(),
            retry_after_secs: None,
            server_resumes: false,
        },
        &mut remote,
    );
    assert!(
        remote.has_loaded_history(),
        "failed restore cannot suppress later prompts"
    );
    assert!(app.dialog.is_some());
    assert!(!app.dialog.as_ref().unwrap().pending_rewind);
    assert!(app.rewind_ui.restore_request.is_none());
}

#[test]
fn checkpoint_rewind_remote_stale_completion_cannot_replace_draft() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    app.remote_session_id = Some("new-session".to_string());
    app.rewind_ui.restore_request =
        Some((17, "old-session".to_string(), RewindMode::ConversationOnly));
    app.input = "new session draft".to_string();
    app.handle_server_event(
        ServerEvent::RewindComplete {
            id: 17,
            session_id: "old-session".to_string(),
            result: rewind_result(RewindMode::ConversationOnly),
        },
        &mut remote,
    );
    assert_eq!(app.input, "new session draft");
    app.remote_session_id = Some("old-session".to_string());
    app.handle_server_event(
        ServerEvent::RewindComplete {
            id: 18,
            session_id: "old-session".to_string(),
            result: rewind_result(RewindMode::ConversationOnly),
        },
        &mut remote,
    );
    assert_eq!(app.input, "new session draft");
    app.handle_server_event(
        ServerEvent::RewindComplete {
            id: 17,
            session_id: "old-session".to_string(),
            result: rewind_result(RewindMode::ConversationOnly),
        },
        &mut remote,
    );
    assert_eq!(app.input, "original 日本語\nsecond line");
    assert!(app.rewind_ui.restore_request.is_none());
}

#[test]
fn checkpoint_rewind_remote_double_escape_opens_same_selector() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    app.remote_session_id = Some("session-rewind".to_string());
    rt.block_on(app.handle_remote_key(KeyCode::Esc, KeyModifiers::empty(), &mut remote))
        .unwrap();
    assert!(app.dialog.is_none());
    rt.block_on(app.handle_remote_key(KeyCode::Esc, KeyModifiers::empty(), &mut remote))
        .unwrap();
    assert!(app.dialog.as_ref().is_some_and(|dialog| dialog.is_rewind()));
    assert!(app.rewind_ui.list_request.is_some());
}

#[test]
fn checkpoint_rewind_local_selector_restores_before_first_user_turn() {
    let mut app = create_test_app();
    let before = app.session.messages.len();
    app.input = "original 日本語\nsecond line".to_string();
    app.submit_input();
    assert!(app.session.messages.len() > before);
    assert_eq!(app.session.rewind_checkpoints().unwrap().len(), 1);
    app.is_processing = false;
    app.pending_turn = false;
    app.provider_session_id = Some("old-provider-session".to_string());
    app.session.provider_session_id = app.provider_session_id.clone();
    app.input = "/rewind".to_string();
    app.submit_input();
    app.handle_key(KeyCode::Up, KeyModifiers::empty()).unwrap();
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();
    assert!(app.rewind_ui.local_restore.is_some());
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(app.process_local_rewind());
    assert_eq!(app.session.messages.len(), before);
    assert!(app.provider_session_id.is_none());
    assert!(app.session.provider_session_id.is_none());
    assert_eq!(app.input, "original 日本語\nsecond line");
    assert!(!app.pending_turn);
    assert!(app.dialog.is_none());
    assert!(app.session.rewind_checkpoints().unwrap().is_empty());
}

#[test]
fn checkpoint_rewind_session_switch_invalidates_pending_response() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    app.remote_session_id = Some("old-session".to_string());
    app.rewind_ui.restore_request =
        Some((17, "old-session".to_string(), RewindMode::ConversationOnly));
    app.dialog = Some(Dialog::rewind_loading());
    app.handle_server_event(
        ServerEvent::SessionId {
            session_id: "new-session".to_string(),
        },
        &mut remote,
    );
    assert!(app.rewind_ui.restore_request.is_none());
    assert!(app.dialog.is_none());
    app.input = "new session draft".to_string();
    app.handle_server_event(
        ServerEvent::RewindComplete {
            id: 17,
            session_id: "old-session".to_string(),
            result: rewind_result(RewindMode::ConversationOnly),
        },
        &mut remote,
    );
    assert_eq!(app.input, "new session draft");
}

#[test]
fn checkpoint_rewind_remote_empty_history_replaces_transcript_before_prompt_refill() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();
    app.remote_session_id = Some("session-first-rewind".to_string());
    app.push_display_message(DisplayMessage::user("removed first prompt"));
    app.push_display_message(DisplayMessage::assistant("removed assistant"));
    app.append_streaming_text("stale assistant tail");
    let id = rt
        .block_on(remote.rewind_checkpoint("first".to_string(), RewindMode::ConversationOnly))
        .unwrap();
    app.rewind_ui.restore_request = Some((
        id,
        "session-first-rewind".to_string(),
        RewindMode::ConversationOnly,
    ));
    app.dialog = Some(Dialog::rewind_loading());
    app.handle_server_event(
        ServerEvent::History {
            id,
            session_id: "session-first-rewind".to_string(),
            messages: Vec::new(),
            images: Vec::new(),
            provider_name: None,
            provider_model: None,
            subagent_model: None,
            agent_model_overrides: Default::default(),
            autoreview_enabled: None,
            autojudge_enabled: None,
            available_models: Vec::new(),
            available_model_routes: Vec::new(),
            mcp_servers: Vec::new(),
            skills: Vec::new(),
            total_tokens: None,
            token_usage_totals: None,
            all_sessions: Vec::new(),
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
            provider_oauth_email: None,
            resolved_credential: None,
            reasoning_effort: None,
            service_tier: None,
            account_labels: Vec::new(),
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: None,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
            applets: Default::default(),
        },
        &mut remote,
    );
    assert!(
        !app.display_messages()
            .iter()
            .any(|message| message.content.contains("removed"))
    );
    assert!(app.streaming_text().is_empty());
    assert!(
        app.input.is_empty(),
        "History cannot refill the prompt before restore completion"
    );
    assert!(app.dialog.is_some());
    app.handle_server_event(
        ServerEvent::RewindComplete {
            id,
            session_id: "session-first-rewind".to_string(),
            result: rewind_result(RewindMode::ConversationOnly),
        },
        &mut remote,
    );
    assert_eq!(app.input, "original 日本語\nsecond line");
    assert!(app.dialog.is_none());
    assert!(!app.pending_turn);
}

#[test]
fn checkpoint_rewind_current_closes_without_changing_conversation_or_draft() {
    let mut app = create_test_app();
    app.input = "authentic user prompt".to_string();
    app.submit_input();
    app.is_processing = false;
    app.pending_turn = false;
    let message_ids = app
        .session
        .messages
        .iter()
        .map(|message| message.id.clone())
        .collect::<Vec<_>>();
    app.open_rewind_dialog_local();
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();
    assert!(app.dialog.is_none());
    assert!(app.rewind_ui.local_restore.is_none());
    assert!(app.input.is_empty());
    assert_eq!(
        app.session
            .messages
            .iter()
            .map(|message| message.id.clone())
            .collect::<Vec<_>>(),
        message_ids
    );
}

#[test]
fn checkpoint_rewind_restores_images_without_claiming_literal_markers_or_submitting() {
    for prompt in [
        "",
        "caption",
        "caption 日本語",
        "caption ",
        "literal [image 1]\ncaption 日本語\n",
    ] {
        let mut app = create_test_app();
        let images = vec![
            ("image/png".to_string(), "original-image-data".to_string()),
            ("image/jpeg".to_string(), "second-image-data".to_string()),
        ];
        let mut result = rewind_result(RewindMode::ConversationOnly);
        result.prompt = prompt.to_string();
        result.images = images.clone();
        app.finish_checkpoint_rewind(result);
        assert_eq!(app.pending_images, images);
        assert_eq!(app.cursor_pos, app.input.len());
        assert!(!app.pending_turn);
        assert!(!app.submit_input_on_startup);
        app.reconcile_input_images();
        let prepared = crate::tui::app::input::take_prepared_input(&mut app);
        assert_eq!(prepared.expanded, prompt);
        assert_eq!(prepared.images, images);
    }
}

#[test]
fn checkpoint_rewind_image_removal_renumbers_owned_separators_without_changing_caption() {
    let mut app = create_test_app();
    let mut result = rewind_result(RewindMode::ConversationOnly);
    result.prompt = "caption 日本語".to_string();
    result.images = vec![
        ("image/png".to_string(), "first-image".to_string()),
        ("image/jpeg".to_string(), "second-image".to_string()),
    ];
    app.finish_checkpoint_rewind(result);
    let start = app.input.rfind("[image 1]").unwrap();
    app.set_input_selection(start, start + "[image 1]".len());
    app.handle_key(KeyCode::Delete, KeyModifiers::empty())
        .unwrap();
    assert_eq!(app.input, "caption 日本語 [image 1]");
    let prepared = crate::tui::app::input::take_prepared_input(&mut app);
    assert_eq!(prepared.expanded, "caption 日本語");
    assert_eq!(
        prepared.images,
        vec![("image/jpeg".to_string(), "second-image".to_string())]
    );
}

#[test]
fn checkpoint_rewind_undo_restores_prior_paste_cursor_and_image_ownership() {
    let mut app = create_test_app();
    app.handle_paste("one\ntwo\nthree\nfour\nfive\nsix".to_string());
    crate::tui::app::input_images::attach_image(
        &mut app,
        "image/png".to_string(),
        "prior-image".to_string(),
    );
    app.cursor_pos = 1;
    let prior_input = app.input.clone();
    let prior_pastes = app.pasted_contents.clone();
    let prior_markers = app.input_image_markers.clone();
    let prior_images = app.pending_images.clone();
    let prior_expanded = crate::tui::app::input::expand_paste_placeholders_with(
        &prior_pastes,
        &app.input_without_owned_image_markers(),
    );
    let mut result = rewind_result(RewindMode::ConversationAndFiles);
    result.images = vec![("image/jpeg".to_string(), "restored-image".to_string())];
    app.finish_checkpoint_rewind(result);
    assert!(app.pasted_contents.is_empty());
    app.undo_input_change();
    assert_eq!(app.input, prior_input);
    assert_eq!(app.cursor_pos, 1);
    assert_eq!(app.pending_images, prior_images);
    assert_eq!(app.input_image_markers, prior_markers);
    assert_eq!(app.pasted_contents, prior_pastes);
    let prepared = crate::tui::app::input::take_prepared_input(&mut app);
    assert_eq!(prepared.expanded, prior_expanded);
    assert_eq!(prepared.images, prior_images);
}

#[test]
fn checkpoint_rewind_files_only_preserves_the_complete_composer() {
    let mut app = create_test_app();
    app.handle_paste("one\ntwo\nthree\nfour\nfive".to_string());
    crate::tui::app::input_images::attach_image(
        &mut app,
        "image/png".to_string(),
        "keep-image".to_string(),
    );
    app.set_input_selection(0, 1);
    let before = (
        app.input.clone(),
        app.cursor_pos,
        app.pending_images.clone(),
        app.input_image_markers.clone(),
        app.pasted_contents.clone(),
        app.input_selection_anchor,
        app.input_undo_stack.len(),
    );
    let mut result = rewind_result(RewindMode::FilesOnly);
    result.images = vec![("image/jpeg".to_string(), "ignore-image".to_string())];
    app.finish_checkpoint_rewind(result);
    assert_eq!(
        (
            app.input.clone(),
            app.cursor_pos,
            app.pending_images.clone(),
            app.input_image_markers.clone(),
            app.pasted_contents.clone(),
            app.input_selection_anchor,
            app.input_undo_stack.len(),
        ),
        before
    );
}

#[test]
fn checkpoint_rewind_real_local_submit_restores_paste_and_image_payload() {
    let mut app = create_test_app();
    let before = app.session.messages.len();
    let text = "first\nsecond\nthird\nfourth\nfifth\n";
    app.handle_paste(text.to_string());
    crate::tui::app::input_images::attach_image(
        &mut app,
        "image/png".to_string(),
        "round-trip-image".to_string(),
    );
    app.submit_input();
    assert!(app.session.messages.len() > before);
    app.is_processing = false;
    app.pending_turn = false;
    app.open_rewind_dialog_local();
    app.handle_key(KeyCode::Up, KeyModifiers::empty()).unwrap();
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(app.process_local_rewind());
    assert_eq!(app.session.messages.len(), before);
    assert!(!app.pending_turn);
    assert!(!app.submit_input_on_startup);
    let prepared = crate::tui::app::input::take_prepared_input(&mut app);
    assert_eq!(prepared.expanded, text);
    assert_eq!(
        prepared.images,
        vec![("image/png".to_string(), "round-trip-image".to_string())]
    );
}

#[test]
fn checkpoint_rewind_assistant_local_picker_keeps_reply_and_exact_composer() {
    let mut app = create_test_app();
    app.input = "identical text".to_string();
    app.submit_input();
    app.is_processing = false;
    app.pending_turn = false;
    let assistant_id = app.session.add_message(
        Role::Assistant,
        vec![ContentBlock::Text {
            text: "identical text".to_string(),
            cache_control: None,
        }],
    );
    let retained = app.session.messages.len();
    app.session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "discard later prompt".to_string(),
            cache_control: None,
        }],
    );
    let original_ids = app
        .session
        .messages
        .iter()
        .map(|message| message.id.clone())
        .collect::<Vec<_>>();
    app.handle_paste("first\nsecond\nthird\nfourth\nfifth\nsixth".to_string());
    crate::tui::app::input_images::attach_image(
        &mut app,
        "image/png".to_string(),
        "keep-image".to_string(),
    );
    app.set_input_selection(0, 1);
    let before = (
        app.input.clone(),
        app.cursor_pos,
        app.pending_images.clone(),
        app.input_image_markers.clone(),
        app.pasted_contents.clone(),
        app.input_selection_anchor,
        app.input_undo_stack.len(),
    );
    app.open_rewind_dialog_local();
    let checkpoints = app.session.rewind_checkpoints().unwrap();
    let assistant = checkpoints
        .iter()
        .find(|row| row.id == format!("assistant:{assistant_id}"))
        .unwrap();
    assert_eq!(assistant.target_kind, RewindTargetKind::AssistantMessage);
    assert!(!assistant.can_restore_files);
    let rows = app.dialog.as_ref().unwrap().item_summaries();
    assert!(
        rows.iter()
            .any(|row| row.starts_with("identical text · You"))
    );
    assert!(
        rows.iter()
            .any(|row| row.starts_with("identical text · Assistant"))
    );
    for key in [KeyCode::Up, KeyCode::Up, KeyCode::Enter, KeyCode::Enter] {
        app.handle_key(key, KeyModifiers::empty()).unwrap();
    }
    assert_eq!(
        app.rewind_ui.local_restore.as_ref().unwrap().0,
        assistant.id
    );
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(app.process_local_rewind());
    assert_eq!(app.session.messages.len(), retained);
    assert_eq!(app.session.messages.last().unwrap().id, assistant_id);
    assert_eq!(
        (
            app.input.clone(),
            app.cursor_pos,
            app.pending_images.clone(),
            app.input_image_markers.clone(),
            app.pasted_contents.clone(),
            app.input_selection_anchor,
            app.input_undo_stack.len()
        ),
        before
    );
    assert!(!app.pending_turn);
    assert!(!app.submit_input_on_startup);
    assert!(app.dialog.is_none());
    assert!(app.rewind_undo_snapshot.is_some());
    app.undo_local_rewind();
    assert_eq!(
        app.session
            .messages
            .iter()
            .map(|message| message.id.clone())
            .collect::<Vec<_>>(),
        original_ids
    );
    let reloaded = crate::session::Session::load(&app.session.id).unwrap();
    assert_eq!(
        reloaded
            .messages
            .iter()
            .map(|message| message.id.clone())
            .collect::<Vec<_>>(),
        original_ids
    );
    assert!(app.provider_session_id.is_none());
    assert!(app.session.provider_session_id.is_none());
    assert!(app.rewind_ui.local_provider_reset_pending);
    assert!(app.rewind_undo_snapshot.is_none());
}

#[test]
fn checkpoint_rewind_assistant_completion_never_uses_assistant_text_or_images_as_draft() {
    let mut app = create_test_app();
    app.handle_paste("one\ntwo\nthree\nfour\nfive\nsix".to_string());
    crate::tui::app::input_images::attach_image(
        &mut app,
        "image/png".to_string(),
        "draft-image".to_string(),
    );
    app.set_input_selection(0, 1);
    let before = (
        app.input.clone(),
        app.cursor_pos,
        app.pending_images.clone(),
        app.input_image_markers.clone(),
        app.pasted_contents.clone(),
        app.input_selection_anchor,
        app.input_undo_stack.len(),
    );
    app.pending_turn = true;
    app.pending_queued_dispatch = true;
    app.submit_input_on_startup = true;
    app.last_submitted_input = Some("discarded continuation".to_string());
    let mut result = rewind_result(RewindMode::ConversationOnly);
    result.target_kind = RewindTargetKind::AssistantMessage;
    result.prompt = "assistant payload must never enter the composer".to_string();
    result.images = vec![("image/jpeg".to_string(), "assistant-image".to_string())];
    app.finish_checkpoint_rewind(result);
    assert_eq!(
        (
            app.input.clone(),
            app.cursor_pos,
            app.pending_images.clone(),
            app.input_image_markers.clone(),
            app.pasted_contents.clone(),
            app.input_selection_anchor,
            app.input_undo_stack.len()
        ),
        before
    );
    assert!(!app.pending_turn);
    assert!(!app.pending_queued_dispatch);
    assert!(!app.submit_input_on_startup);
    assert!(app.last_submitted_input.is_none());
}

#[test]
fn checkpoint_rewind_last_assistant_keeps_current_conversation_and_draft() {
    let mut app = create_test_app();
    app.session.add_message(
        Role::Assistant,
        vec![ContentBlock::Text {
            text: "last assistant reply".to_string(),
            cache_control: None,
        }],
    );
    let ids = app
        .session
        .messages
        .iter()
        .map(|message| message.id.clone())
        .collect::<Vec<_>>();
    app.input = "keep this draft".to_string();
    app.cursor_pos = 3;
    app.open_rewind_dialog_local();
    for key in [KeyCode::Up, KeyCode::Enter, KeyCode::Enter] {
        app.handle_key(key, KeyModifiers::empty()).unwrap();
    }
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(app.process_local_rewind());
    assert_eq!(
        app.session
            .messages
            .iter()
            .map(|message| message.id.clone())
            .collect::<Vec<_>>(),
        ids
    );
    assert_eq!(app.input, "keep this draft");
    assert_eq!(app.cursor_pos, 3);
    assert!(!app.pending_turn);
    assert!(!app.submit_input_on_startup);
    assert!(app.dialog.is_none());
}

#[test]
fn checkpoint_rewind_assistant_queue_cleanup_does_not_repair_current_image_markers() {
    let mut app = create_test_app();
    app.input = "literal [image 1]".to_string();
    app.set_input_selection(0, 2);
    let anchor = app.input_selection_anchor;
    app.pending_images
        .push(("image/png".to_string(), "legacy-image".to_string()));
    let before = (app.input.clone(), app.input_image_markers.clone());
    app.queued_messages.push("queued human draft".to_string());
    let mut result = rewind_result(RewindMode::ConversationOnly);
    result.target_kind = RewindTargetKind::AssistantMessage;
    app.finish_checkpoint_rewind(result);
    assert_eq!((app.input.clone(), app.input_image_markers.clone()), before);
    assert_eq!(app.cursor_pos, 2);
    assert_eq!(app.input_selection_anchor, anchor);
    assert_eq!(
        app.pending_images,
        vec![("image/png".to_string(), "legacy-image".to_string())]
    );
    assert!(app.queued_messages.is_empty());
    assert!(!app.pending_turn);
    app.undo_input_change();
    assert_eq!(app.input, "queued human draft");
}
