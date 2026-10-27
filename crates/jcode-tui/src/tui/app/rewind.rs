//! Checkpoint rewind UI state shared by the local and wire clients.
use super::*;
use crate::protocol::{RewindCheckpoint, RewindMode, RewindResult, RewindTargetKind, ServerEvent};
use crate::tui::backend::{RemoteConnection, RemoteEventState};
use crate::tui::dialog::Dialog;

#[derive(Default)]
pub(super) struct RewindUiState {
    pub last_empty_escape: Option<Instant>,
    pub list_request: Option<(u64, String)>,
    pub restore_request: Option<(u64, String, RewindMode)>,
    pub local_restore: Option<(String, RewindMode)>,
    pub local_provider_reset_pending: bool,
}

impl App {
    pub(super) fn conversation_rewind_history_pending(&self, session_id: &str) -> bool {
        self.checkpoint_conversation_restore_pending(session_id)
            || self.pending_remote_rewind_notice.is_some()
    }

    pub(super) fn checkpoint_conversation_restore_pending(&self, session_id: &str) -> bool {
        self.rewind_ui
            .restore_request
            .as_ref()
            .is_some_and(|(_, session, mode)| mode.restores_conversation() && session == session_id)
    }

    pub(super) fn finish_legacy_rewind_notice(&mut self) {
        if let Some(notice) = self.pending_remote_rewind_notice.take() {
            let content = if notice.undo {
                "✓ Undid rewind. Restored the messages removed by the last rewind.".to_string()
            } else {
                format!(
                    "✓ Rewound to message {}. Removed {} message{}. Undo anytime with /rewind undo.",
                    notice.message_index.unwrap_or_default(),
                    notice.changed_messages,
                    if notice.changed_messages == 1 {
                        ""
                    } else {
                        "s"
                    }
                )
            };
            self.push_display_message(DisplayMessage::system(content));
        }
    }

    pub(super) fn invalidate_rewind_ui(&mut self) {
        self.rewind_ui = RewindUiState {
            local_provider_reset_pending: self.rewind_ui.local_provider_reset_pending,
            ..Default::default()
        };
        if self
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.is_rewind())
        {
            self.dialog = None;
        }
    }
    pub(super) fn bind_local_rewind_checkpoint(&mut self, checkpoint_id: Option<String>) {
        let Some(checkpoint_id) = checkpoint_id else {
            return;
        };
        let Some(message_id) = self
            .session
            .messages
            .last()
            .map(|message| message.id.clone())
        else {
            return;
        };
        if let Err(error) = self
            .session
            .bind_rewind_checkpoint(&checkpoint_id, &message_id)
        {
            self.push_display_message(DisplayMessage::error(format!(
                "Could not bind rewind checkpoint: {error}"
            )));
        }
    }
    /// Called only after modal/selection/cancellation handling has declined the key.
    pub(super) fn rewind_empty_escape(&mut self, previous: Option<Instant>, now: Instant) -> bool {
        if self.is_processing || !self.input.is_empty() || !self.pending_images.is_empty() {
            return false;
        }
        if previous
            .is_some_and(|at| now.saturating_duration_since(at) <= Duration::from_millis(500))
        {
            self.rewind_ui.last_empty_escape = None;
            true
        } else {
            self.rewind_ui.last_empty_escape = Some(now);
            self.set_status_notice("Press Esc again to rewind");
            false
        }
    }

    pub(super) fn open_rewind_dialog_local(&mut self) {
        self.rewind_ui.last_empty_escape = None;
        self.dialog = Some(Dialog::rewind_loading());
        if self.is_remote {
            self.rewind_dialog_error("Reconnect to the server to load checkpoints.".to_string());
            return;
        }
        match self.session.rewind_checkpoints() {
            Ok(checkpoints) => self.apply_rewind_checkpoints(checkpoints),
            Err(error) => self.rewind_dialog_error(error),
        }
    }

    pub(super) fn apply_rewind_checkpoints(&mut self, checkpoints: Vec<RewindCheckpoint>) {
        if let Some(dialog) = self.dialog.as_mut().filter(|dialog| dialog.is_rewind()) {
            dialog.set_rewind_checkpoints(checkpoints);
        }
    }

    pub(super) fn rewind_dialog_error(&mut self, error: String) {
        if let Some(dialog) = self.dialog.as_mut().filter(|dialog| dialog.is_rewind()) {
            dialog.pending_rewind = false;
            dialog.set_status(Some(error));
        }
    }

    pub(super) async fn process_local_rewind(&mut self) {
        let Some((checkpoint_id, mode)) = self.rewind_ui.local_restore.take() else {
            return;
        };
        if self.is_processing || self.pending_turn {
            self.rewind_dialog_error(
                "Wait for the current turn to finish before rewinding.".to_string(),
            );
            return;
        }
        let previous_message_count = self.session.messages.len();
        let undo = (mode == RewindMode::ConversationOnly
            && checkpoint_id.starts_with("assistant:"))
        .then(|| LocalRewindUndoSnapshot {
            messages: self.session.messages.clone(),
            visible_message_count: self.session.rewind_target_count(),
        });
        let result = self.session.rewind_checkpoint(&checkpoint_id, mode).await;
        self.apply_local_rewind_result(previous_message_count, result, undo)
            .await;
    }

    pub(super) async fn apply_local_rewind_result(
        &mut self,
        previous_message_count: usize,
        result: std::result::Result<RewindResult, String>,
        undo: Option<LocalRewindUndoSnapshot>,
    ) {
        let conversation_changed = self.session.messages.len() != previous_message_count;
        let reset_result = if conversation_changed {
            self.reconcile_local_rewind_conversation();
            // Install before awaiting provider reset: cancellation or a reset
            // error must not lose undo for an already committed truncation.
            if self.session.messages.len() < previous_message_count {
                self.rewind_undo_snapshot = undo;
            }
            self.ensure_local_rewind_provider_reset().await
        } else {
            Ok(())
        };
        match result {
            Ok(result) => {
                self.finish_checkpoint_rewind(result);
                if let Err(error) = reset_result {
                    self.set_status_notice(format!("Conversation restored; {error}"));
                }
            }
            Err(error) => {
                let error = match reset_result {
                    Ok(()) => error,
                    Err(reset_error) => format!("{error}; {reset_error}"),
                };
                self.rewind_dialog_error(error);
            }
        }
    }

    pub(super) fn rewind_local_to_message(&mut self, message_index: usize) {
        let targets = self.session.rewind_target_stored_indices();
        let Some(&stored_index) = message_index
            .checked_sub(1)
            .and_then(|index| targets.get(index))
        else {
            self.push_display_message(DisplayMessage::error(format!(
                "Invalid message number: {}. Valid range: 1-{}",
                message_index,
                targets.len()
            )));
            return;
        };
        let snapshot = LocalRewindUndoSnapshot {
            messages: self.session.messages.clone(),
            visible_message_count: targets.len(),
        };
        let removed = targets.len() - message_index;
        let previous_len = self.session.messages.len();
        let result = self.session.rewind_conversation_to_len(stored_index + 1);
        if self.session.messages.len() < previous_len {
            self.reconcile_local_rewind_conversation();
            self.rewind_undo_snapshot = Some(snapshot);
        }
        match result {
            Ok(()) => self.push_display_message(DisplayMessage::system(format!(
                "✓ Rewound to message {}. Removed {} message{}. Undo anytime with /rewind undo.",
                message_index,
                removed,
                if removed == 1 { "" } else { "s" }
            ))),
            Err(error) => self.push_display_message(DisplayMessage::error(format!(
                "Could not finish conversation rewind: {error:#}"
            ))),
        }
    }

    pub(super) fn undo_local_rewind(&mut self) {
        let Some(snapshot) = self.rewind_undo_snapshot.take() else {
            self.push_display_message(DisplayMessage::system("No rewind to undo.".to_string()));
            return;
        };
        let restored = snapshot
            .visible_message_count
            .saturating_sub(self.session.rewind_target_count());
        let previous_ids = self
            .session
            .messages
            .iter()
            .map(|message| message.id.clone())
            .collect::<Vec<_>>();
        let result = self
            .session
            .restore_rewind_messages(snapshot.messages.clone());
        let changed = !self
            .session
            .messages
            .iter()
            .map(|message| &message.id)
            .eq(previous_ids.iter());
        if result.is_err() && !changed {
            self.rewind_undo_snapshot = Some(snapshot);
        } else {
            self.reconcile_local_rewind_conversation();
        }
        match result {
            Ok(()) => self.push_display_message(DisplayMessage::system(format!(
                "✓ Undid rewind. Restored {} message{}.",
                restored,
                if restored == 1 { "" } else { "s" }
            ))),
            Err(error) => self.push_display_message(DisplayMessage::error(format!(
                "Could not finish rewind undo: {error:#}"
            ))),
        }
    }

    fn reconcile_local_rewind_conversation(&mut self) {
        self.provider_session_id = None;
        self.session.provider_session_id = None;
        self.rewind_undo_snapshot = None;
        self.rewind_ui.local_provider_reset_pending = true;
        self.clear_rewind_continuation_state();
        let provider_messages = self.session.messages_for_provider_uncached();
        self.replace_provider_messages(provider_messages);
        self.clear_display_messages();
        self.stream_buffer.clear();
        self.clear_streaming_render_state();
        self.streaming_tool_calls.clear();
        let restored = crate::session::render_messages(&self.session)
            .into_iter()
            .map(|rendered| DisplayMessage {
                role: rendered.role,
                content: rendered.content,
                tool_calls: rendered.tool_calls,
                duration_secs: None,
                title: None,
                tool_data: rendered.tool_data,
            })
            .collect();
        self.replace_display_messages(restored);
        self.follow_chat_bottom();
    }

    pub(super) async fn ensure_local_rewind_provider_reset(&mut self) -> Result<()> {
        if self.rewind_ui.local_provider_reset_pending {
            self.provider
                .reset_conversation_state()
                .await
                .map_err(|error| {
                    anyhow::anyhow!("provider reset failed, retry before continuing: {error}")
                })?;
            self.rewind_ui.local_provider_reset_pending = false;
        }
        Ok(())
    }

    pub(super) fn clear_rewind_continuation_state(&mut self) {
        self.preserve_rewind_queued_drafts();
        self.clear_pending_remote_retry();
        self.clear_pending_fallback_offer();
        self.last_submitted_input = None;
        self.pending_turn = false;
        self.pending_queued_dispatch = false;
        self.account_change_resend_at = None;
        self.local_usage_limit_resume_attempts = 0;
        self.current_turn_system_reminder = None;
        self.submit_input_on_startup = false;
        self.compacted_history_lazy = Default::default();
        self.pending_history_anchor = None;
        self.pending_resize_anchor = None;
    }

    fn preserve_rewind_queued_drafts(&mut self) {
        let mut prompts = std::mem::take(&mut self.pending_soft_interrupts);
        self.pending_soft_interrupt_requests.clear();
        if let Some(prompt) = self.interleave_message.take() {
            prompts.push(prompt);
        }
        prompts.append(&mut self.queued_messages);
        prompts.retain(|prompt| !super::commands::is_poke_message(prompt));
        let mut images = std::mem::take(&mut self.interleave_images);
        images.append(&mut self.queued_message_images);
        let commands = std::mem::take(&mut self.queued_commands);
        if !prompts.is_empty() || !images.is_empty() || !commands.is_empty() {
            // Saving queued drafts must not normalize legacy image markers in
            // the live composer. Assistant restores leave that draft exact.
            self.input_typing_undo = None;
            let draft = self.input_draft_snapshot();
            if self.input_undo_stack.last() != Some(&draft)
                || self.input_undo_image_counts.last() != Some(&self.pending_images.len())
            {
                self.push_input_undo_snapshot(draft);
            }
            for command in commands {
                self.save_rewind_queued_draft(command, Vec::new());
            }
            if !prompts.is_empty() || !images.is_empty() {
                self.save_rewind_queued_draft(prompts.join("\n\n"), images);
            }
        }
        self.hidden_queued_system_messages.clear();
        self.remote_interrupt_ack_deadline = None;
        self.queued_followup_starved_since = None;
    }

    fn save_rewind_queued_draft(&mut self, text: String, images: Vec<(String, String)>) {
        let image_count = images.len();
        let cursor = text.len();
        self.push_input_undo_snapshot((
            text,
            cursor,
            std::sync::Arc::new(images),
            Default::default(),
            Default::default(),
        ));
        if let Some(count) = self.input_undo_image_counts.last_mut() {
            *count = image_count;
        }
    }

    pub(super) fn finish_checkpoint_rewind(&mut self, result: RewindResult) {
        self.dialog = None;
        self.rewind_ui.last_empty_escape = None;
        if result.mode.restores_conversation() {
            self.clear_rewind_continuation_state();
        }
        if result.mode.restores_conversation() && result.target_kind == RewindTargetKind::UserPrompt
        {
            self.remember_input_undo_state();
            let cursor = result.prompt.len();
            self.restore_input_draft((
                result.prompt,
                cursor,
                std::sync::Arc::new(result.images),
                Default::default(),
                Default::default(),
            ));
            self.cursor_pos = self.input.len();
            self.input_selection_anchor = None;
            self.history_draft = None;
            self.prompt_history_search = None;
            self.reset_tab_completion();
            self.sync_model_picker_preview_from_input();
            self.submit_input_on_startup = false;
            self.follow_chat_bottom();
        }
        let mut notice = format!(
            "Restored checkpoint: {} message(s) removed, {} file(s) restored.",
            result.removed_messages, result.restored_files
        );
        if !result.skipped_files.is_empty() {
            notice.push_str(&format!(
                " {} file(s) skipped: {}",
                result.skipped_files.len(),
                result.skipped_files.join(", ")
            ));
        }
        self.set_status_notice(notice);
    }
}

pub(super) async fn open_remote(app: &mut App, remote: &mut RemoteConnection) {
    app.rewind_ui.last_empty_escape = None;
    app.dialog = Some(Dialog::rewind_loading());
    let session_id = app.remote_session_id.clone().unwrap_or_default();
    match remote.list_rewind_checkpoints().await {
        Ok(id) => app.rewind_ui.list_request = Some((id, session_id)),
        Err(error) => app.rewind_dialog_error(format!("Could not list checkpoints: {error}")),
    }
}

pub(super) async fn restore_remote(
    app: &mut App,
    remote: &mut RemoteConnection,
    checkpoint_id: String,
    mode: RewindMode,
) {
    if app.is_processing {
        app.rewind_dialog_error(
            "Wait for the current turn to finish before rewinding.".to_string(),
        );
        return;
    }
    let session_id = app.remote_session_id.clone().unwrap_or_default();
    match remote.rewind_checkpoint(checkpoint_id, mode).await {
        Ok(id) => app.rewind_ui.restore_request = Some((id, session_id, mode)),
        Err(error) => app.rewind_dialog_error(format!("Could not restore checkpoint: {error}")),
    }
}

/// Checkpoint replies belong only to the requesting window and session.
pub(super) fn handle_checkpoint_event(
    app: &mut App,
    event: &ServerEvent,
    remote: &mut impl RemoteEventState,
) -> bool {
    match event {
        ServerEvent::RewindCheckpoints {
            id,
            session_id,
            checkpoints,
        } => {
            if app
                .rewind_ui
                .list_request
                .as_ref()
                .is_some_and(|(request, session)| {
                    request == id
                        && session == session_id
                        && app.remote_session_id.as_ref() == Some(session_id)
                })
            {
                app.rewind_ui.list_request = None;
                app.apply_rewind_checkpoints(checkpoints.clone());
            }
            true
        }
        ServerEvent::RewindComplete {
            id,
            session_id,
            result,
        } => {
            if app
                .rewind_ui
                .restore_request
                .as_ref()
                .is_some_and(|(request, session, mode)| {
                    request == id
                        && session == session_id
                        && *mode == result.mode
                        && app.remote_session_id.as_ref() == Some(session_id)
                })
            {
                app.rewind_ui.restore_request = None;
                if result.mode.restores_conversation() {
                    remote.clear_pending();
                    remote.reset_call_output_tokens_seen();
                }
                app.finish_checkpoint_rewind(result.clone());
            }
            true
        }
        ServerEvent::Error { id, message, .. }
            if app
                .rewind_ui
                .list_request
                .as_ref()
                .is_some_and(|(request, _)| request == id)
                || app
                    .rewind_ui
                    .restore_request
                    .as_ref()
                    .is_some_and(|(request, _, _)| request == id) =>
        {
            let same_session = app
                .rewind_ui
                .list_request
                .as_ref()
                .map(|(_, session)| session)
                .or_else(|| {
                    app.rewind_ui
                        .restore_request
                        .as_ref()
                        .map(|(_, session, _)| session)
                })
                == app.remote_session_id.as_ref();
            let was_restore = app.rewind_ui.restore_request.take();
            app.rewind_ui.list_request = None;
            if same_session {
                // Rejected restores send no History, so release the bootstrap gate.
                if was_restore.is_some_and(|(_, _, mode)| mode.restores_conversation()) {
                    remote.mark_history_loaded();
                }
                app.rewind_dialog_error(message.clone());
            }
            true
        }
        _ => false,
    }
}
