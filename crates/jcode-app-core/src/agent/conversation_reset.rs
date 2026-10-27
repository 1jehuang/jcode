use super::*;

impl Agent {
    pub(super) fn reset_runtime_state_for_session_change(&mut self) {
        self.active_skill = None;
        self.last_upstream_provider = None;
        self.last_connection_type = None;
        self.last_status_detail = None;
        self.pending_alerts.clear();
        self.current_turn_system_reminder = None;
        self.reset_tool_output_tracking();
        if let Ok(mut queue) = self.soft_interrupt_queue.lock() {
            queue.clear();
        }
        self.background_tool_signal.reset();
        self.graceful_shutdown.reset();
        self.cache_tracker.reset();
        self.kv_cache_monitor.reset();
        self.last_usage = TokenUsage::default();
        self.locked_tools = None;
        self.mcp_late_register_resolved = false;
        self.rewind_undo_snapshot = None;
        self.preserve_compaction_on_provider_reset = false;
    }

    pub(super) fn checkpoint_user_input(
        &mut self,
        prompt: &str,
        has_images: bool,
        synthetic: bool,
    ) -> Result<Option<String>> {
        if !synthetic && crate::session::is_authentic_user_input(prompt, has_images) {
            Ok(Some(self.session.create_rewind_checkpoint(prompt)?.id))
        } else {
            Ok(None)
        }
    }

    pub(super) fn bind_user_checkpoint(&mut self, checkpoint_id: Option<String>) -> Result<()> {
        if let Some(checkpoint_id) = checkpoint_id {
            let input_id = self
                .session
                .messages
                .last()
                .expect("user input was appended")
                .id
                .clone();
            self.session
                .bind_rewind_checkpoint(&checkpoint_id, &input_id)?;
        }
        Ok(())
    }

    pub fn rewind_checkpoints(&self) -> Result<Vec<jcode_session_types::RewindCheckpoint>, String> {
        self.session.rewind_checkpoints()
    }

    pub async fn rewind_checkpoint(
        &mut self,
        checkpoint_id: &str,
        mode: jcode_session_types::RewindMode,
    ) -> Result<jcode_session_types::RewindResult, String> {
        let previous_len = self.session.messages.len();
        let undo = (mode == jcode_session_types::RewindMode::ConversationOnly
            && checkpoint_id.starts_with("assistant:"))
        .then(|| self.capture_rewind_undo_snapshot());
        let result = self.session.rewind_checkpoint(checkpoint_id, mode).await;
        if mode.restores_conversation() && self.session.messages.len() < previous_len {
            self.provider_session_id = None;
            self.reset_runtime_state_for_session_change();
            // The broad reset clears the old slot. Assistant rewind only changes
            // the transcript, so its retained sidecar metadata can support undo.
            self.rewind_undo_snapshot = undo;
            // Mark this before awaiting: errors or cancellation must prevent a
            // later turn from resuming the provider's discarded branch.
            self.provider_conversation_reset_pending = true;
            let provider_reset = self.ensure_provider_conversation_reset().await;
            if let Err(error) = provider_reset {
                return Err(match result {
                    Ok(_) => format!("Conversation rewound, but provider reset failed: {error:#}"),
                    Err(original) => format!("{original}; provider reset failed: {error:#}"),
                });
            }
        }
        result
    }

    fn capture_rewind_undo_snapshot(&self) -> RewindUndoSnapshot {
        RewindUndoSnapshot {
            messages: self.session.messages.clone(),
            visible_message_count: self.session.rewind_target_count(),
        }
    }

    pub(super) async fn ensure_provider_conversation_reset(&mut self) -> Result<()> {
        if self.provider_conversation_reset_pending {
            // This is part of the retry gate: cancellation while waiting for a
            // compaction reader must not expose a summary of discarded history.
            if !self.preserve_compaction_on_provider_reset {
                let compaction = self.registry.compaction();
                let mut manager = compaction.write().await;
                manager.reset();
                manager.set_budget(self.provider.input_token_limit());
                manager.seed_restored_stored_messages_with(&self.session.messages);
            }
            self.provider.reset_conversation_state().await?;
            self.provider_conversation_reset_pending = false;
            self.preserve_compaction_on_provider_reset = false;
        }
        Ok(())
    }

    /// Clear conversation history
    pub fn clear(&mut self) {
        let preserve_canary = self.session.is_canary;
        let preserve_testing_build = self.session.testing_build.clone();
        let preserve_debug = self.session.is_debug;
        let preserve_working_dir = self.session.working_dir.clone();

        self.session.mark_closed();
        self.finish_concurrency_tracking();
        self.persist_session_best_effort("pre-clear session close state");

        let mut new_session = Session::create(None, None);
        new_session.mark_active();
        new_session.model = Some(self.provider_model());
        new_session.provider_key = self.provider_key_for_new_session();
        new_session.is_canary = preserve_canary;
        new_session.testing_build = preserve_testing_build;
        new_session.is_debug = preserve_debug;
        new_session.working_dir = preserve_working_dir;
        new_session.ensure_initial_session_context_message();

        self.session = new_session;
        self.begin_concurrency_tracking();
        self._tool_policy_registration = crate::tool::register_session_tool_policy(
            &self.session.id,
            self.allowed_tools.clone(),
            self.disabled_tools.clone(),
        );
        self.refresh_agents_md_snapshot();
        self.reconcile_explicit_provider_pin_route();
        self.reset_runtime_state_for_session_change();
        self.provider_session_id = None;
        self.seed_compaction_from_session();
        self.provider_conversation_reset_pending = true;
    }

    /// Clear provider session so the next turn sends full context.
    pub fn reset_provider_session(&mut self) {
        self.provider_session_id = None;
        self.session.provider_session_id = None;
        self.provider_conversation_reset_pending = true;
        self.persist_session_best_effort("provider session reset");
    }

    /// Rewind the conversation to a 1-based visible transcript message index.
    ///
    /// The index is interpreted against the same rendered transcript the TUI
    /// numbers in `/rewind` (user/assistant entries only, tool cards and
    /// system notices excluded). Mapping through raw stored messages instead
    /// would count tool-result messages the UI never numbers, sending
    /// `/rewind N` far earlier than the on-screen message N (issue #432).
    ///
    /// Provider-side resumable sessions are reset so the next request sends the
    /// truncated context from scratch instead of continuing from a stale upstream
    /// conversation.
    pub fn rewind_to_message(&mut self, message_index: usize) -> Result<usize, String> {
        let message_count = self.session.rewind_target_count();
        let stored_len = self
            .session
            .rewind_stored_len_for_message(message_index)
            .ok_or_else(|| {
                format!(
                    "Invalid message number: {}. Valid range: 1-{}",
                    message_index, message_count
                )
            })?;

        let removed = message_count - message_index;
        let previous_len = self.session.messages.len();
        let undo = self.capture_rewind_undo_snapshot();
        let result = self.session.rewind_conversation_to_len(stored_len);
        if self.session.messages.len() < previous_len {
            self.provider_session_id = None;
            self.reset_runtime_state_for_session_change();
            self.rewind_undo_snapshot = Some(undo);
            self.provider_conversation_reset_pending = true;
        }
        result.map_err(|error| format!("{error:#}"))?;
        Ok(removed)
    }

    pub fn undo_rewind(&mut self) -> Result<usize, String> {
        let Some(snapshot) = self.rewind_undo_snapshot.clone() else {
            return Err("No rewind to undo.".to_string());
        };

        let current_count = self.session.rewind_target_count();
        let restored = snapshot.visible_message_count.saturating_sub(current_count);
        let previous_ids: Vec<_> = self.session.messages.iter().map(|m| m.id.clone()).collect();
        let result = self.session.restore_rewind_messages(snapshot.messages);
        let changed = !self
            .session
            .messages
            .iter()
            .map(|m| &m.id)
            .eq(previous_ids.iter());
        if result.is_ok() || changed {
            // Never resume the old provider branch, even when undo restores its
            // transcript. Reset also consumes the one-slot in-memory snapshot.
            self.provider_session_id = None;
            self.reset_runtime_state_for_session_change();
            self.provider_conversation_reset_pending = true;
        }
        result.map_err(|error| format!("{error:#}"))?;
        Ok(restored)
    }
}
