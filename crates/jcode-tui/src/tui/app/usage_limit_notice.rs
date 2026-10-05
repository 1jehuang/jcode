//! Usage-limit holds that resume by themselves.
//!
//! Remote, server-initiated turns: the server resumes scheduled tasks, swarm
//! wakes and DM turns on its own when a subscription usage limit resets. It
//! tells attached clients with `ServerEvent::Error { id: 0,
//! retry_after_secs: Some(..) }`. The client did not send that turn, so it
//! only settles its view of the turn and shows when the server resumes. It
//! must not hold or resend anything itself.
//!
//! Local turns: a user-typed turn that hits a usage limit is held and runs
//! again after the reset (the remote client already does this for its own
//! turns).

use super::{App, DisplayMessage, ProcessingStatus};
use std::time::{Duration, Instant};

impl App {
    /// Local mode: hold a turn that hit a subscription usage limit and run it
    /// again after the reset. Returns false when `error` is not a usage limit
    /// with a known reset, or this is a remote client (the remote error path
    /// owns its hold).
    pub(super) fn hold_local_turn_for_usage_limit(&mut self, error: &str) -> bool {
        if self.is_remote {
            return false;
        }
        let Some(reset_in) = jcode_provider_core::usage_limit_resume::usage_limit_hint_from_text(
            error,
            chrono::Utc::now().timestamp(),
        )
        .and_then(|hint| hint.reset_in) else {
            return false;
        };
        // Never retry immediately: a reset that already passed waits the
        // same minimum as the server-side resume.
        let wait = jcode_provider_core::usage_limit_resume::usage_limit_resume_delay(
            Some(reset_in),
            Duration::ZERO,
        );
        // The prompt is already in the transcript and runs again from there;
        // do not also restore it to the input box.
        self.last_submitted_input = None;
        self.rate_limit_reset = Some(Instant::now() + wait);
        let notice = self.rate_limit_notice_with_nudge(wait.as_secs());
        self.push_display_message(DisplayMessage::error(error.to_string()));
        self.push_display_message(DisplayMessage::system(notice));
        self.set_status_notice("Usage limit; will retry at the reset");
        true
    }

    /// Text shown when the server will resume a turn in `resume_in_secs`.
    pub(super) fn server_usage_limit_resume_notice(resume_in_secs: u64) -> String {
        let at = chrono::Local::now() + chrono::Duration::seconds(resume_in_secs as i64);
        format!(
            "⏳ Usage limit hit. The server will resume this at {}",
            at.format("%H:%M")
        )
    }

    pub(super) fn handle_server_owned_usage_limit_resume(&mut self, resume_in_secs: u64) {
        let notice = Self::server_usage_limit_resume_notice(resume_in_secs);
        crate::logging::info(&format!(
            "Server-initiated turn hit a usage limit; server resumes in {resume_in_secs}s"
        ));
        self.push_display_message(DisplayMessage::system(notice));
        self.set_status_notice("Usage limit; server will resume");
        // Settle the adopted turn. The server owns the resume, so do not arm
        // a client-side resend (rate_limit_reset / rate_limit_pending_message
        // stay as they are: a held turn of the user's own is unaffected).
        crate::tui::mermaid::clear_streaming_preview_diagram();
        self.is_processing = false;
        self.status = ProcessingStatus::Idle;
        self.stream_message_ended = false;
        self.processing_started = None;
        self.remote_resume_activity = None;
        self.streaming_tool_calls.clear();
        self.thought_line_inserted = false;
        self.thinking_prefix_emitted = false;
        self.thinking_buffer.clear();
        self.clear_visible_turn_started();
    }
}
