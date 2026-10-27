use super::*;
use crate::protocol::{RewindMode, RewindResult, RewindTargetKind, ServerEvent};
use crate::tui::dialog::Dialog;
use std::sync::atomic::{AtomicUsize, Ordering};

struct ResetRecordingProvider {
    resets: AtomicUsize,
    failures: usize,
}

#[async_trait::async_trait]
impl Provider for ResetRecordingProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        panic!("rewind never submits a provider request")
    }

    async fn reset_conversation_state(&self) -> Result<()> {
        let attempt = self.resets.fetch_add(1, Ordering::SeqCst);
        anyhow::ensure!(attempt >= self.failures, "injected provider reset failure");
        Ok(())
    }

    fn name(&self) -> &str {
        "rewind-test"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            resets: AtomicUsize::new(0),
            failures: self.failures,
        })
    }
}

fn arm_stale_continuation(app: &mut App) {
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "removed continuation".to_string(),
        images: Vec::new(),
        is_system: false,
        system_reminder: None,
        auto_retry: true,
        retry_attempts: 1,
        retry_at: None,
        overload_attempts: 0,
    });
    app.last_submitted_input = Some("removed continuation".to_string());
    app.account_change_resend_at = Some(Instant::now());
    app.pending_queued_dispatch = true;
    app.submit_input_on_startup = true;
    app.local_usage_limit_resume_attempts = 1;
    app.hidden_queued_system_messages
        .push("discarded hidden reminder".to_string());
    app.queued_messages.push("human queued prompt".to_string());
    app.interleave_message = Some("human steering prompt".to_string());
    app.queued_message_images
        .push(("image/png".to_string(), "queued-image".to_string()));
    app.queued_followup_starved_since = Some(Instant::now() - Duration::from_secs(60));
    app.remote_interrupt_ack_deadline = Some(Instant::now());
}

fn assert_continuation_cleared(app: &App) {
    assert!(app.rate_limit_pending_message.is_none());
    assert!(app.last_submitted_input.is_none());
    assert!(app.account_change_resend_at.is_none());
    assert!(!app.pending_queued_dispatch);
    assert!(!app.submit_input_on_startup);
    assert_eq!(app.local_usage_limit_resume_attempts, 0);
    assert!(!app.has_queued_followups());
    assert!(app.queued_message_images.is_empty());
    assert!(app.interleave_images.is_empty());
    assert!(app.queued_followup_starved_since.is_none());
    assert!(app.remote_interrupt_ack_deadline.is_none());
}

#[test]
fn rewind_local_committed_error_reconciles_transcript_and_retries_provider_reset() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app();
    let provider = Arc::new(ResetRecordingProvider {
        resets: AtomicUsize::new(0),
        failures: 2,
    });
    app.provider = provider.clone();
    app.input = "removed authentic prompt".to_string();
    app.submit_input();
    app.is_processing = false;
    app.pending_turn = false;
    let previous_count = app.session.messages.len();
    let checkpoint = app.session.rewind_checkpoints().unwrap().pop().unwrap();
    // Perform the real durable truncation, then exercise the post-commit error
    // result boundary. The base crate separately injects finalization failure.
    rt.block_on(
        app.session
            .rewind_checkpoint(&checkpoint.id, RewindMode::ConversationOnly),
    )
    .unwrap();
    app.input = "keep this editable draft".to_string();
    app.cursor_pos = 4;
    app.provider_session_id = Some("discarded-provider-cursor".to_string());
    app.session.provider_session_id = app.provider_session_id.clone();
    app.append_streaming_text("removed assistant tail");
    app.dialog = Some(Dialog::rewind_loading());
    arm_stale_continuation(&mut app);
    rt.block_on(app.apply_local_rewind_result(
        previous_count,
        Err("injected post-commit finalization error".to_string()),
        None,
    ));
    assert!(app.session.messages.len() < previous_count);
    assert_eq!(
        serde_json::to_value(app.materialized_provider_messages()).unwrap(),
        serde_json::to_value(app.session.messages_for_provider_uncached()).unwrap()
    );
    assert!(
        !app.display_messages()
            .iter()
            .any(|m| m.content.contains("removed authentic prompt"))
    );
    assert!(app.streaming_text().is_empty());
    assert!(app.provider_session_id.is_none());
    assert!(app.session.provider_session_id.is_none());
    assert_continuation_cleared(&app);
    assert_eq!(app.input, "keep this editable draft");
    assert_eq!(app.cursor_pos, 4);
    assert!(app.dialog.is_some());
    assert_eq!(provider.resets.load(Ordering::SeqCst), 1);
    assert!(app.rewind_ui.local_provider_reset_pending);
    app.invalidate_rewind_ui();
    assert!(app.rewind_ui.local_provider_reset_pending);
    assert!(
        rt.block_on(app.ensure_local_rewind_provider_reset())
            .is_err()
    );
    assert!(app.rewind_ui.local_provider_reset_pending);
    rt.block_on(app.ensure_local_rewind_provider_reset())
        .unwrap();
    assert!(!app.rewind_ui.local_provider_reset_pending);
    assert_eq!(provider.resets.load(Ordering::SeqCst), 3);
    rt.block_on(app.ensure_local_rewind_provider_reset())
        .unwrap();
    assert_eq!(provider.resets.load(Ordering::SeqCst), 3);
}

#[test]
fn rewind_local_rejection_preserves_continuation_and_does_not_reset_provider() {
    let mut app = create_test_app();
    let provider = Arc::new(ResetRecordingProvider {
        resets: AtomicUsize::new(0),
        failures: 0,
    });
    app.provider = provider.clone();
    arm_stale_continuation(&mut app);
    let count = app.session.messages.len();
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(app.apply_local_rewind_result(
            count,
            Err("checkpoint not found".to_string()),
            None,
        ));
    assert!(app.rate_limit_pending_message.is_some());
    assert!(app.account_change_resend_at.is_some());
    assert!(app.submit_input_on_startup);
    assert_eq!(provider.resets.load(Ordering::SeqCst), 0);
}

fn empty_history(id: u64, session_id: &str) -> ServerEvent {
    ServerEvent::History {
        id,
        session_id: session_id.to_string(),
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
        side_panel: Default::default(),
        applets: Default::default(),
    }
}

#[test]
fn rewind_remote_history_then_error_clears_stale_continuation_but_rejection_does_not() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    for committed in [true, false] {
        let mut app = create_test_app();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();
        app.remote_session_id = Some("rewind-error-session".to_string());
        app.input = "keep original editor".to_string();
        app.cursor_pos = 3;
        app.dialog = Some(Dialog::rewind_loading());
        app.push_display_message(DisplayMessage::user("removed future"));
        arm_stale_continuation(&mut app);
        let id = rt
            .block_on(remote.rewind_checkpoint("target".to_string(), RewindMode::ConversationOnly))
            .unwrap();
        app.rewind_ui.restore_request = Some((
            id,
            "rewind-error-session".to_string(),
            RewindMode::ConversationOnly,
        ));
        if committed {
            app.handle_server_event(empty_history(id, "rewind-error-session"), &mut remote);
            assert_continuation_cleared(&app);
            assert!(
                !app.display_messages()
                    .iter()
                    .any(|m| m.content == "removed future")
            );
        }
        app.handle_server_event(
            ServerEvent::Error {
                id,
                message: "restore failed".to_string(),
                retry_after_secs: None,
                server_resumes: false,
            },
            &mut remote,
        );
        assert_eq!(app.input, "keep original editor");
        assert_eq!(app.cursor_pos, 3);
        assert!(app.dialog.is_some());
        assert!(app.rewind_ui.restore_request.is_none());
        if committed {
            assert_continuation_cleared(&app);
            app.undo_input_change();
            assert!(app.input.contains("human queued prompt"));
            assert!(app.input.contains("human steering prompt"));
            assert!(!app.input.contains("discarded hidden reminder"));
            assert_eq!(
                app.pending_images,
                vec![("image/png".to_string(), "queued-image".to_string())]
            );
            assert!(!app.pending_turn);
            assert!(!app.has_queued_followups());
        } else {
            assert!(app.rate_limit_pending_message.is_some());
            assert!(app.account_change_resend_at.is_some());
            assert!(app.submit_input_on_startup);
            assert!(app.pending_queued_dispatch);
            assert!(app.has_queued_followups());
        }
    }
}

#[test]
fn rewind_remote_assistant_completion_keeps_composer_and_rejects_stale_replies() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut app = create_test_app();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();
    app.remote_session_id = Some("assistant-rewind".to_string());
    app.handle_paste("one\ntwo\nthree\nfour\nfive\nsix".to_string());
    crate::tui::app::input_images::attach_image(
        &mut app,
        "image/png".to_string(),
        "keep-draft-image".to_string(),
    );
    app.set_input_selection(0, 1);
    let before = (
        app.input.clone(),
        app.cursor_pos,
        app.pending_images.clone(),
        app.input_image_markers.clone(),
        app.pasted_contents.clone(),
        app.input_selection_anchor,
    );
    arm_stale_continuation(&mut app);
    app.dialog = Some(Dialog::rewind_loading());
    let id = rt
        .block_on(
            remote.rewind_checkpoint("assistant:reply".to_string(), RewindMode::ConversationOnly),
        )
        .unwrap();
    app.rewind_ui.restore_request = Some((
        id,
        "assistant-rewind".to_string(),
        RewindMode::ConversationOnly,
    ));
    let result = RewindResult {
        target_kind: RewindTargetKind::AssistantMessage,
        mode: RewindMode::ConversationOnly,
        prompt: "assistant text is never a user draft".to_string(),
        images: vec![(
            "image/jpeg".to_string(),
            "ignore-assistant-image".to_string(),
        )],
        removed_messages: 2,
        restored_files: 0,
        skipped_files: Vec::new(),
    };
    for (reply_id, session, mode) in [
        (id + 1, "assistant-rewind", RewindMode::ConversationOnly),
        (id, "other-session", RewindMode::ConversationOnly),
        (id, "assistant-rewind", RewindMode::FilesOnly),
    ] {
        let mut stale = result.clone();
        stale.mode = mode;
        app.handle_server_event(
            ServerEvent::RewindComplete {
                id: reply_id,
                session_id: session.to_string(),
                result: stale,
            },
            &mut remote,
        );
        assert!(app.rewind_ui.restore_request.is_some());
        assert!(app.dialog.is_some());
        assert_eq!(app.input, before.0);
    }
    app.handle_server_event(empty_history(id, "assistant-rewind"), &mut remote);
    assert!(remote.has_loaded_history());
    assert_continuation_cleared(&app);
    assert!(
        app.dialog.is_some(),
        "History alone must not complete the restore"
    );
    app.handle_server_event(
        ServerEvent::RewindComplete {
            id,
            session_id: "assistant-rewind".to_string(),
            result: result.clone(),
        },
        &mut remote,
    );
    assert_eq!(
        (
            app.input.clone(),
            app.cursor_pos,
            app.pending_images.clone(),
            app.input_image_markers.clone(),
            app.pasted_contents.clone(),
            app.input_selection_anchor
        ),
        before
    );
    assert_continuation_cleared(&app);
    assert!(!app.pending_turn);
    assert!(app.dialog.is_none());
    assert!(app.rewind_ui.restore_request.is_none());
    let undo_count = app.input_undo_stack.len();
    app.handle_server_event(
        ServerEvent::RewindComplete {
            id,
            session_id: "assistant-rewind".to_string(),
            result,
        },
        &mut remote,
    );
    assert_eq!(app.input_undo_stack.len(), undo_count);
    assert_eq!(app.input, before.0);
}

#[test]
fn rewind_local_assistant_reset_failure_keeps_undo_and_undo_requires_fresh_provider_reset() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app();
    let provider = Arc::new(ResetRecordingProvider {
        resets: AtomicUsize::new(0),
        failures: 2,
    });
    app.provider = provider.clone();
    let text = |text: &str| ContentBlock::Text {
        text: text.to_string(),
        cache_control: None,
    };
    app.session
        .add_message(Role::User, vec![text("original prompt")]);
    let assistant_id = app
        .session
        .add_message(Role::Assistant, vec![text("keep reply")]);
    app.session
        .add_message(Role::User, vec![text("removed future")]);
    let original_ids = app
        .session
        .messages
        .iter()
        .map(|message| message.id.clone())
        .collect::<Vec<_>>();
    app.provider_session_id = Some("do-not-resume-old-provider".to_string());
    app.session.provider_session_id = app.provider_session_id.clone();
    app.input = "draft survives restore and undo".to_string();
    app.cursor_pos = 4;
    app.rewind_ui.local_restore = Some((
        format!("assistant:{assistant_id}"),
        RewindMode::ConversationOnly,
    ));
    rt.block_on(app.process_local_rewind());
    assert!(app.rewind_undo_snapshot.is_some());
    assert!(app.rewind_ui.local_provider_reset_pending);
    assert_eq!(provider.resets.load(Ordering::SeqCst), 1);
    app.undo_local_rewind();
    assert!(app.rewind_undo_snapshot.is_none());
    assert_eq!(
        app.session
            .messages
            .iter()
            .map(|message| message.id.clone())
            .collect::<Vec<_>>(),
        original_ids
    );
    assert!(app.provider_session_id.is_none());
    assert!(app.session.provider_session_id.is_none());
    assert!(app.rewind_ui.local_provider_reset_pending);
    assert!(
        rt.block_on(app.ensure_local_rewind_provider_reset())
            .is_err()
    );
    assert!(app.rewind_ui.local_provider_reset_pending);
    rt.block_on(app.ensure_local_rewind_provider_reset())
        .unwrap();
    assert!(!app.rewind_ui.local_provider_reset_pending);
    assert_eq!(app.input, "draft survives restore and undo");
    assert_eq!(app.cursor_pos, 4);
    assert!(!app.pending_turn);
    assert!(!app.submit_input_on_startup);
}

#[test]
fn rewind_local_numeric_keeps_inclusive_rendered_boundary_and_durably_undoes() {
    let mut app = create_test_app();
    let text = |text: &str| ContentBlock::Text {
        text: text.to_string(),
        cache_control: None,
    };
    app.session
        .add_message(Role::User, vec![text("first prompt")]);
    let assistant_id = app
        .session
        .add_message(Role::Assistant, vec![text("selected assistant")]);
    app.session
        .add_message(Role::User, vec![text("later prompt")]);
    let original_ids = app
        .session
        .messages
        .iter()
        .map(|message| message.id.clone())
        .collect::<Vec<_>>();
    let stored_index = app
        .session
        .messages
        .iter()
        .position(|message| message.id == assistant_id)
        .unwrap();
    let target = app
        .session
        .rewind_target_stored_indices()
        .iter()
        .position(|&index| index == stored_index)
        .unwrap()
        + 1;
    app.input = "keep draft".to_string();
    app.cursor_pos = 2;
    arm_stale_continuation(&mut app);
    app.rewind_local_to_message(target);
    assert_eq!(app.session.messages.last().unwrap().id, assistant_id);
    assert_eq!(app.session.messages.len(), stored_index + 1);
    assert!(app.rewind_ui.local_provider_reset_pending);
    assert_continuation_cleared(&app);
    assert_eq!(app.input, "keep draft");
    assert_eq!(app.cursor_pos, 2);
    assert!(!app.pending_turn);
    let saved_ids = app
        .rewind_undo_snapshot
        .as_ref()
        .unwrap()
        .messages
        .iter()
        .map(|message| message.id.clone())
        .collect::<Vec<_>>();
    assert_eq!(saved_ids, original_ids);
    app.rewind_local_to_message(target); // Tail no-op cannot replace the prior undo slot.
    app.rewind_local_to_message(usize::MAX); // Validation failure cannot consume it either.
    assert_eq!(
        app.rewind_undo_snapshot.as_ref().unwrap().messages.len(),
        original_ids.len()
    );
    app.undo_local_rewind();
    let reloaded = crate::session::Session::load(&app.session.id).unwrap();
    assert_eq!(
        reloaded
            .messages
            .iter()
            .map(|message| message.id.clone())
            .collect::<Vec<_>>(),
        original_ids
    );
    assert!(app.rewind_undo_snapshot.is_none());
    assert!(app.provider_session_id.is_none());
    assert!(app.session.provider_session_id.is_none());
    assert!(app.rewind_ui.local_provider_reset_pending);
}

#[test]
fn rewind_remote_legacy_numeric_and_undo_history_clear_continuations_without_touching_draft() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    for undo in [false, true] {
        let mut app = create_test_app();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();
        app.is_remote = true;
        app.remote_session_id = Some("legacy-rewind-cleanup".to_string());
        app.handle_paste("one\ntwo\nthree\nfour\nfive\nsix".to_string());
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
        );
        arm_stale_continuation(&mut app);
        let auto_poke = crate::todo::build_auto_poke_message(3);
        app.queued_messages.push(auto_poke.clone());
        app.auto_poke_incomplete_todos = true;
        app.append_streaming_text("discard stale assistant stream");
        let id = if undo {
            rt.block_on(remote.rewind_undo()).unwrap()
        } else {
            rt.block_on(remote.rewind(2)).unwrap()
        };
        app.pending_remote_rewind_notice = Some(PendingRemoteRewindNotice {
            undo,
            message_index: (!undo).then_some(2),
            changed_messages: 1,
        });
        let mut history = empty_history(id, "legacy-rewind-cleanup");
        if let ServerEvent::History { messages, .. } = &mut history {
            messages.push(crate::protocol::HistoryMessage {
                response_stats: None,
                role: "assistant".to_string(),
                content: "retained assistant".to_string(),
                tool_calls: None,
                tool_data: None,
            });
        }
        app.handle_server_event(history, &mut remote);
        assert_continuation_cleared(&app);
        assert!(!app.pending_turn);
        assert!(!app.has_queued_followups());
        assert!(app.streaming_text().is_empty());
        assert!(app.pending_remote_rewind_notice.is_none());
        assert!(remote.has_loaded_history());
        assert_eq!(
            (
                app.input.clone(),
                app.cursor_pos,
                app.pending_images.clone(),
                app.input_image_markers.clone(),
                app.pasted_contents.clone(),
                app.input_selection_anchor
            ),
            before
        );
        app.undo_input_change();
        assert!(app.input.contains("human queued prompt"));
        assert!(app.input.contains("human steering prompt"));
        assert!(!app.input.contains(&auto_poke));
        assert!(!app.submit_input_on_startup);
    }
}
