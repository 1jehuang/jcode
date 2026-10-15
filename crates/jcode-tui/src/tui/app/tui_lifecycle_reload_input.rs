use super::*;

impl App {
    pub(in crate::tui::app) fn apply_restored_reload_input(
        &mut self,
        restored: RestoredReloadInput,
    ) {
        self.input = restored.input;
        self.cursor_pos = restored.cursor;
        self.pending_images = restored.pending_images;
        self.ensure_input_image_markers();
        self.submit_input_on_startup = restored.submit_on_restore
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
        if let Some(status_notice) = restored.startup_status_notice {
            self.set_status_notice(status_notice);
        } else if self.submit_input_on_startup {
            self.set_status_notice("Startup prompt queued");
        }
        if let Some((title, message)) = restored.startup_display_message {
            self.push_display_message(DisplayMessage::system(message).with_title(title));
        }
        self.interleave_message = None;
        self.interleave_images.clear();
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
        self.todo_final_response_requested = self.final_response_todo_fingerprint.is_some();

        let mut queued_messages = restored.queued_messages;
        let mut recovered_followups = Vec::new();
        if let Some(interleave_message) = restored.interleave_message
            && !interleave_message.trim().is_empty()
        {
            recovered_followups.push(interleave_message);
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
