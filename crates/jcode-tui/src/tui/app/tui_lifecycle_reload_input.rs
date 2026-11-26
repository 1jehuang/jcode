use super::*;

pub(in crate::tui::app) struct RestoredReloadInput {
    pub input: String,
    pub cursor: usize,
    pub pasted_contents: Vec<String>,
    pub image_markers: super::input_images::InputImageMarkers,
    pub pending_images: Vec<(String, String)>,
    pub submit_on_restore: bool,
    pub queued_messages: Vec<String>,
    pub queued_images: Vec<(String, String)>,
    pub queued_commands: Vec<String>,
    pub hidden_queued_system_messages: Vec<String>,
    pub startup_status_notice: Option<String>,
    pub startup_display_message: Option<(String, String)>,
    pub interleave_message: Option<String>,
    pub interleave_images: Vec<(String, String)>,
    pub pending_soft_interrupts: Vec<String>,
    pub pending_soft_interrupt_resend: Option<Vec<String>>,
    pub rate_limit_pending_message: Option<super::PendingRemoteMessage>,
    pub last_submitted_draft: Option<super::input::PreparedInput>,
    pub rate_limit_reset: Option<Instant>,
    pub observe_mode_enabled: bool,
    pub observe_page_markdown: String,
    pub observe_page_updated_at_ms: u64,
    pub split_view_enabled: bool,
    pub todos_view_enabled: bool,
    pub todo_confidence_spike_challenged: bool,
    pub last_todo_ownership_fingerprint: Option<String>,
    pub final_response_todo_fingerprint: Option<String>,
    pub last_auto_poke_fingerprint: Option<String>,
    pub auto_poke_attempts: u8,
}

impl App {
    /// A pending reload-recovery continuation must run as its own turn before
    /// user follow-ups queued ahead of the reload are sent.
    pub(in crate::tui::app) fn reload_recovery_blocks_queue(&self) -> bool {
        self.reload_recovery_dispatch_pending && !self.hidden_queued_system_messages.is_empty()
    }

    pub(in crate::tui::app) fn apply_restored_reload_input(
        &mut self,
        restored: RestoredReloadInput,
    ) {
        self.input = restored.input;
        self.cursor_pos = super::input::floor_char_boundary(&self.input, restored.cursor);
        self.pending_images = restored.pending_images;
        self.input_image_markers = restored.image_markers;
        self.pasted_contents = restored.pasted_contents;
        self.ensure_input_image_markers();
        let command_shaped =
            self.input.trim_start().starts_with('!') || self.input.trim_start().starts_with('/');
        let proven_draft = restored.last_submitted_draft.as_ref().is_some_and(|draft| {
            draft.raw_input == self.input
                && draft.images == self.pending_images
                && draft.image_markers == self.input_image_markers
                && draft.pasted_contents == self.pasted_contents
        });
        let review_required = restored.submit_on_restore
            && !self.pending_images.is_empty()
            && command_shaped
            && !proven_draft;
        if restored.submit_on_restore
            && !self.pending_images.is_empty()
            && !review_required
            && !proven_draft
        {
            let prompt = self.input_without_owned_image_markers();
            self.input.clear();
            self.restore_recalled_prompt_text(prompt);
        }
        self.submit_input_on_startup = restored.submit_on_restore
            && !review_required
            && (!self.input.is_empty() || !self.pending_images.is_empty());
        crate::logging::info(&format!(
            "Startup input restored: submit_on_restore={} input_chars={} pending_images={} queued_messages={} hidden_system={} => submit_input_on_startup={}",
            restored.submit_on_restore,
            self.input.chars().count(),
            self.pending_images.len(),
            restored.queued_messages.len(),
            restored.hidden_queued_system_messages.len(),
            self.submit_input_on_startup,
        ));
        self.hidden_queued_system_messages = restored.hidden_queued_system_messages;
        if review_required {
            self.set_status_notice("Restored image prompt needs review before sending; its original command intent could not be verified");
        } else if let Some(status_notice) = restored.startup_status_notice {
            self.set_status_notice(status_notice);
        } else if self.submit_input_on_startup {
            self.set_status_notice("Startup prompt queued");
        }
        if let Some((title, message)) = restored.startup_display_message {
            self.push_display_message(DisplayMessage::system(message).with_title(title));
        }
        self.interleave_message = None;
        self.interleave_images.clear();
        let held_draft = restored.last_submitted_draft.filter(|_| {
            restored.rate_limit_pending_message.is_some()
                || (self.submit_input_on_startup && proven_draft)
        });
        self.last_submitted_input = held_draft.as_ref().map(|draft| draft.raw_input.clone());
        self.last_submitted_draft = held_draft.map(|draft| (self.current_message_id, draft));
        self.rate_limit_pending_message = restored.rate_limit_pending_message;
        self.rate_limit_reset = restored.rate_limit_reset;
        self.observe_page_markdown = restored.observe_page_markdown;
        self.observe_page_updated_at_ms = restored.observe_page_updated_at_ms;
        self.set_observe_mode_enabled(restored.observe_mode_enabled, restored.observe_mode_enabled);
        self.set_split_view_enabled(restored.split_view_enabled, restored.split_view_enabled);
        self.set_todos_view_enabled(restored.todos_view_enabled, restored.todos_view_enabled);
        self.todo_confidence_spike_challenged = restored.todo_confidence_spike_challenged;
        self.last_todo_ownership_fingerprint = restored.last_todo_ownership_fingerprint;
        self.final_response_todo_fingerprint = restored.final_response_todo_fingerprint;
        self.last_auto_poke_fingerprint = restored.last_auto_poke_fingerprint;
        self.auto_poke_attempts = restored.auto_poke_attempts;
        self.todo_final_response_requested = self.final_response_todo_fingerprint.is_some();

        let mut queued_messages = restored.queued_messages;
        let mut queued_images = if queued_messages.is_empty() {
            Vec::new()
        } else {
            restored.queued_images
        };
        let mut recovered_followups = Vec::new();
        if let Some(interleave_message) = restored.interleave_message
            && (!interleave_message.trim().is_empty() || !restored.interleave_images.is_empty())
        {
            recovered_followups.push(interleave_message);
            queued_images.splice(0..0, restored.interleave_images);
        }
        let recovered_interrupts = restored
            .pending_soft_interrupt_resend
            .unwrap_or(restored.pending_soft_interrupts);
        if !recovered_interrupts.is_empty() {
            crate::logging::info(&format!(
                "Recovered {} pending soft interrupt(s) after reload; re-queueing them as normal follow-ups",
                recovered_interrupts.len()
            ));
            recovered_followups.extend(recovered_interrupts);
        }
        if !recovered_followups.is_empty() {
            let mut recovered_queue = recovered_followups;
            recovered_queue.append(&mut queued_messages);
            queued_messages = recovered_queue;
            self.set_status_notice("Recovered pending prompts after reload");
        }

        self.queued_messages = queued_messages;
        self.queued_images = queued_images;
        self.queued_commands = restored.queued_commands;
        if self.has_queued_followups() {
            if self.is_remote {
                // Do not synthesize a processing turn for restored remote follow-ups.
                // After a reload, the server may still be running the previous turn;
                // the queue must remain a wait-until-turn-end queue until the history
                // bootstrap/Done event proves the remote turn is idle. The remote
                // post-connect/history/tick paths will dispatch once it is safe.
                self.set_status_notice("Restored queued follow-up after reload");
            } else {
                self.is_processing = true;
                self.status = ProcessingStatus::Sending;
                if self.processing_started.is_none() {
                    self.processing_started = Some(Instant::now());
                }
                self.pending_turn = true;
            }
        }
    }
}

impl RestoredReloadInput {
    pub(in crate::tui::app) fn submitted_draft_from_saved(
        value: &serde_json::Value,
        rate_limit_pending_message: Option<&super::PendingRemoteMessage>,
        input: &str,
        pending_images: &[(String, String)],
    ) -> Option<super::input::PreparedInput> {
        match value.get("last_submitted_draft") {
            Some(value) if !value.is_null() => {
                match serde_json::from_value::<super::input::PreparedInput>(value.clone()) {
                    Ok(draft) => Some(draft),
                    Err(error) => {
                        crate::logging::warn(&format!("Invalid saved submitted draft: {error}"));
                        None
                    }
                }
            }
            _ => None,
        }
        .filter(|draft| {
            if let Some(pending) = rate_limit_pending_message {
                !pending.is_system
                    && draft.expanded == pending.content
                    && draft.images == pending.images
            } else {
                value
                    .get("submit_on_restore")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true)
                    && draft.raw_input == input
                    && draft.images == pending_images
            }
        })
    }
}
