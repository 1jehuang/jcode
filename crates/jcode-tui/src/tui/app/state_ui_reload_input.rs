//! Composer persistence across reloads.

use super::*;

impl App {
    pub(in super::super) fn save_input_for_reload(&self, session_id: &str) {
        let resume_prompt = self.rate_limit_pending_message.as_ref().filter(|pending| {
            !pending.auto_retry
                && !pending.is_system
                && (!pending.content.trim().is_empty() || !pending.images.is_empty())
        });
        // A queued follow-up that was dequeued and is currently in flight lives
        // only in `rate_limit_pending_message` (is_system). Without a scheduled
        // retry reset, that shape has no dispatch path after a restore (the
        // tick resend requires `rate_limit_reset`), so persist it back into
        // the queued/hidden lists instead; the restored queue re-sends it once
        // the turn is proven idle (issue #391).
        let inflight_continuation = self.rate_limit_pending_message.as_ref().filter(|pending| {
            pending.is_system
                && self.rate_limit_reset.is_none()
                && (!pending.content.trim().is_empty() || pending.system_reminder.is_some())
        });
        if self.input.is_empty()
            && self.pending_images.is_empty()
            && self.queued_messages.is_empty()
            && self.queued_commands.is_empty()
            && self.hidden_queued_system_messages.is_empty()
            && self.interleave_message.is_none()
            && self.pending_soft_interrupts.is_empty()
            && self.pending_soft_interrupt_requests.is_empty()
            && self.rate_limit_pending_message.is_none()
            && resume_prompt.is_none()
            && !self.observe_mode_enabled
            && !self.split_view_enabled
            && !self.todos_view_enabled
            && !self.todo_confidence_spike_challenged
            && self.last_todo_ownership_fingerprint.is_none()
            && self.final_response_todo_fingerprint.is_none()
            && self.last_auto_poke_fingerprint.is_none()
        {
            // Nothing to save, but a stale file from an earlier run could
            // still hold old queued messages/input. Leaving it behind would
            // resurrect that stale state on the next restore. Only remove
            // clearly stale files: another client attached to the same session
            // may have just saved ITS queued messages during the same reload
            // handoff, and deleting a fresh file here would drop them.
            if let Ok(jcode_dir) = crate::storage::jcode_dir() {
                let path = jcode_dir.join(format!("client-input-{}", session_id));
                let is_stale = std::fs::metadata(&path)
                    .and_then(|meta| meta.modified())
                    .ok()
                    .and_then(|mtime| mtime.elapsed().ok())
                    .is_some_and(|age| age > Duration::from_secs(300));
                if is_stale {
                    let _ = std::fs::remove_file(&path);
                }
            }
            return;
        }
        if let Ok(jcode_dir) = crate::storage::jcode_dir() {
            let path = jcode_dir.join(format!("client-input-{}", session_id));
            let rate_limit_reset_in_ms =
                if resume_prompt.is_some() || inflight_continuation.is_some() {
                    None
                } else {
                    self.rate_limit_reset.map(|reset| {
                        let now = Instant::now();
                        if reset <= now {
                            0
                        } else {
                            (reset - now).as_millis().min(u64::MAX as u128) as u64
                        }
                    })
                };
            let rate_limit_pending_message =
                if resume_prompt.is_some() || inflight_continuation.is_some() {
                    None
                } else {
                    self.rate_limit_pending_message.as_ref().map(|pending| {
                        serde_json::json!({
                            "content": pending.content,
                            "images": pending.images,
                            "is_system": pending.is_system,
                            "system_reminder": pending.system_reminder,
                            "auto_retry": pending.auto_retry,
                            "retry_attempts": pending.retry_attempts,
                        })
                    })
                };
            let mut queued_messages = self.queued_messages.clone();
            let mut hidden_queued_system_messages = self.hidden_queued_system_messages.clone();
            if let Some(pending) = inflight_continuation {
                if !pending.content.trim().is_empty() {
                    queued_messages.insert(0, pending.content.clone());
                }
                if let Some(reminder) = pending.system_reminder.clone() {
                    hidden_queued_system_messages.insert(0, reminder);
                }
            }
            let resume_input = resume_prompt.map(|pending| pending.content.as_str());
            let resume_images = resume_prompt.map(|pending| pending.images.as_slice());
            let rate_limit_reset_in_ms =
                rate_limit_reset_in_ms.or_else(|| resume_prompt.map(|_| 0));
            let pending_soft_interrupt_resend = self
                .pending_soft_interrupt_requests
                .iter()
                .map(|(_, content)| content.clone())
                .collect::<Vec<_>>();
            let data = serde_json::json!({
                "cursor": resume_input.map(|input| input.len()).unwrap_or(self.cursor_pos),
                "input": resume_input.unwrap_or(self.input.as_str()),
                "pending_images": resume_images.unwrap_or(self.pending_images.as_slice()).iter().map(|(media_type, data)| serde_json::json!({
                    "media_type": media_type,
                    "data": data,
                })).collect::<Vec<_>>(),
                "submit_on_restore": resume_prompt.is_some(),
                "queued_messages": queued_messages,
                "queued_commands": self.queued_commands,
                "hidden_queued_system_messages": hidden_queued_system_messages,
                "interleave_message": self.interleave_message,
                "pending_soft_interrupts": self.pending_soft_interrupts,
                "pending_soft_interrupt_resend": pending_soft_interrupt_resend,
                "rate_limit_pending_message": rate_limit_pending_message,
                "rate_limit_reset_in_ms": rate_limit_reset_in_ms,
                "observe_mode_enabled": self.observe_mode_enabled,
                "observe_page_markdown": self.observe_page_markdown,
                "observe_page_updated_at_ms": self.observe_page_updated_at_ms,
                "split_view_enabled": self.split_view_enabled,
                "todos_view_enabled": self.todos_view_enabled,
                "todo_confidence_spike_challenged": self.todo_confidence_spike_challenged,
                "last_todo_ownership_fingerprint": self.last_todo_ownership_fingerprint,
                "final_response_todo_fingerprint": self.final_response_todo_fingerprint,
                "last_auto_poke_fingerprint": self.last_auto_poke_fingerprint,
                "auto_poke_attempts": self.auto_poke_attempts,
            });
            let _ = std::fs::write(&path, data.to_string());
        }
    }
}
