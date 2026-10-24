//! Conservative TUI hibernation: retain source data, release derived data.
//!
//! Remote GetHistory is not a lossless UI snapshot: it replaces the transcript
//! and cannot restore local-only messages, expanded regions or a reading anchor.
//! Keep those authoritative values (including drafts and attachments) untouched.
//! A bounded copy of the last terminal frame lets us drop expensive prepared
//! transcripts and decoded images without rebuilding them on every idle tick.

use super::*;
use ratatui::buffer::Buffer;

// Terminal cells can contain image-protocol strings, not just graphemes. Bound
// both the cell storage and the variable-length symbols before cloning a frame.
const MAX_FROZEN_CELLS: usize = 128 * 1024;
const MAX_FROZEN_SYMBOL_BYTES: usize = 8 * 1024 * 1024;

pub(super) struct IdleHibernation {
    last_busy: Instant,
    pending: bool,
    pub(super) frame: Option<Buffer>,
    pub(super) notice: String,
    rss_before: Option<u64>,
    rss_after: Option<u64>,
    measure_after_frame: bool,
    notice_dirty: bool,
    /// Transcript mode released the transcript; it must be refetched on wake.
    transcript_dropped: bool,
    pub(super) history_reload_needed: bool,
    wake_count: u64,
    hibernation_count: u64,
    last_wake_reason: Option<String>,
}

impl Default for IdleHibernation {
    fn default() -> Self {
        Self {
            last_busy: Instant::now(),
            pending: false,
            frame: None,
            notice: String::new(),
            rss_before: None,
            rss_after: None,
            measure_after_frame: false,
            notice_dirty: false,
            transcript_dropped: false,
            history_reload_needed: false,
            wake_count: 0,
            hibernation_count: 0,
            last_wake_reason: None,
        }
    }
}

impl App {
    /// Rechecked at the point of eviction, not just when scheduling a frame.
    fn hibernation_has_work(&self) -> bool {
        self.is_processing()
            || self.pending_turn
            || self.session_save_pending
            || !self.queued_messages.is_empty()
            || !self.queued_commands.is_empty()
            || !self.hidden_queued_system_messages.is_empty()
            || !self.streaming.streaming_text.is_empty()
            || !self.stream_buffer.is_empty()
            || !self.streaming_tool_calls.is_empty()
            || self.deferred_stream_done_id.is_some()
            || self.current_message_id.is_some()
            || self.remote_resume_activity.is_some()
            || self.pending_prompt_before_history.is_some()
            || self.pending_prompt_after_model_switch.is_some()
            || self.submit_input_on_startup
            || self.pending_provider_failover.is_some()
            || self.pending_fallback_resend.is_some()
            || self.rate_limit_pending_message.is_some()
            || self.rate_limit_reset.is_some()
            || self.interleave_message.is_some()
            || !self.pending_soft_interrupts.is_empty()
            || !self.pending_soft_interrupt_requests.is_empty()
            || self.pending_local_transfer.is_some()
            || self.pending_transfer_request
            || self.pending_split_request
            || self.pending_split_prompt.is_some()
            || self.pending_catchup_resume.is_some()
            || self.in_flight_catchup_resume.is_some()
            || self.remote_history_wait_started.is_some()
            || self.remote_model_switch_in_flight
            || self.pending_model_switch.is_some()
            || self.pending_route_selection.is_some()
            || self.pending_background_client_reload.is_some()
            || self.pending_server_reload
            || self.pending_migration.is_some()
            || self.voice_input.is_some()
            || self.dictation_in_flight
            || self.remote_login.is_some()
            || self.pending_login.is_some()
            || self.usage_reset.is_running()
            || self
                .background_task_rows
                .iter()
                .any(|task| task.status == crate::tui::BackgroundTaskRowStatus::Running)
            || (!self.is_remote
                && self
                    .registry
                    .compaction()
                    .try_read()
                    .map_or(true, |manager| manager.is_compacting()))
    }

    pub(super) fn poll_idle_hibernation(&mut self) -> bool {
        let idle_secs = crate::config::config().session.hibernation_idle_secs;
        self.poll_idle_hibernation_at(Instant::now(), idle_secs)
    }

    fn poll_idle_hibernation_at(&mut self, now: Instant, idle_secs: u64) -> bool {
        if idle_secs == 0 || self.hibernation_has_work() || self.is_replay {
            let was_hibernated = self.idle_hibernation.frame.is_some();
            self.wake_idle_hibernation_for("disabled_or_work_pending");
            self.idle_hibernation.last_busy = now;
            return was_hibernated;
        }
        if self.idle_hibernation.frame.is_some() {
            return std::mem::take(&mut self.idle_hibernation.notice_dirty);
        }
        if self.idle_hibernation.pending {
            return true;
        }
        let last_activity = [
            Some(self.idle_hibernation.last_busy),
            self.last_user_interaction,
            self.last_stream_activity,
            Some(self.app_started),
        ]
        .into_iter()
        .flatten()
        .max()
        .unwrap_or(now);
        self.idle_hibernation.pending =
            now.saturating_duration_since(last_activity) >= Duration::from_secs(idle_secs);
        if self.idle_hibernation.pending {
            // The animation-only fast path never enters draw_full, where the
            // completed frame is captured and caches can safely be released.
            self.force_full_repaint = true;
        }
        self.idle_hibernation.pending
    }

    /// Called by the normal renderer with its last *completed* terminal frame.
    /// Nothing is evicted if there is no safe, bounded frame to keep on screen.
    pub(super) fn enter_idle_hibernation(&mut self, previous_frame: Option<&Buffer>) {
        if !self.idle_hibernation.pending || self.hibernation_has_work() {
            return;
        }
        self.idle_hibernation.pending = false;
        let Some(frame) = previous_frame.filter(|frame| frame_can_be_frozen(frame)) else {
            // Do not retry an oversized frame on every deep-idle tick.
            self.idle_hibernation.last_busy = Instant::now();
            return;
        };
        let before = private_memory_bytes();
        let mode = crate::config::config().session.hibernation_mode;
        if mode == jcode_config_types::HibernationMode::Exit
            && self.is_remote
            && !crate::tui::is_ssh_remote()
            && let Some(session_id) = self.remote_session_id.clone()
        {
            // The daemon owns the session, so the client can exit and a tiny
            // waiter process resumes it on the next key press.
            crate::logging::info("TUI hibernation: exiting client process");
            self.save_input_for_reload(&session_id);
            self.hibernate_exit_requested = Some(session_id);
            self.should_quit = true;
            return;
        }
        if mode == jcode_config_types::HibernationMode::Transcript
            && self.is_remote
            && self.remote_session_id.is_some()
            && !self.display_messages.is_empty()
        {
            // Must run before the frame is frozen: clearing bumps the
            // display version, which is itself a wake signal.
            self.clear_display_messages();
            self.display_messages.shrink_to_fit();
            self.remote_side_pane_images = Vec::new();
            self.invalidate_side_pane_images_signature();
            self.idle_hibernation.transcript_dropped = true;
        }
        self.idle_hibernation.frame = Some(frame.clone());
        self.idle_hibernation.hibernation_count += 1;
        self.session.release_provider_messages_cache();
        self.streaming_md_renderer
            .replace(IncrementalMarkdownRenderer::new(None));
        self.agent_edited_cache.replace(None);
        self.command_suggestions_cache.replace(None);
        crate::tui::ui::release_idle_render_caches();
        crate::process_memory::release_retained_heap("tui_hibernation");
        let after = private_memory_bytes();
        self.idle_hibernation.rss_before = before;
        self.idle_hibernation.rss_after = after;
        self.idle_hibernation.notice = hibernation_notice(before, after);
        self.idle_hibernation.measure_after_frame = true;
    }

    /// Sampling after the first frozen frame includes ratatui's working buffers
    /// and the renderer's retained frame, not just the transient eviction point.
    pub(super) fn finish_idle_hibernation_frame(&mut self) {
        if self.idle_hibernation.frame.is_some()
            && std::mem::take(&mut self.idle_hibernation.measure_after_frame)
        {
            let after = private_memory_bytes();
            self.idle_hibernation.rss_after = after;
            self.idle_hibernation.notice =
                hibernation_notice(self.idle_hibernation.rss_before, after);
            self.idle_hibernation.notice_dirty = true;
            crate::logging::info(&self.idle_hibernation.notice);
        }
    }

    /// Runs before handling the original event. It never consumes or rewrites
    /// that event and never touches the draft, cursor, scroll or session state.
    #[track_caller]
    pub(super) fn wake_idle_hibernation(&mut self) {
        if self.idle_hibernation.pending || self.idle_hibernation.frame.is_some() {
            let caller = std::panic::Location::caller();
            self.wake_idle_hibernation_for(&format!("{}:{}", caller.file(), caller.line()));
        } else {
            self.idle_hibernation.last_busy = Instant::now();
        }
    }

    pub(super) fn wake_idle_hibernation_for(&mut self, reason: &str) {
        self.idle_hibernation.last_busy = Instant::now();
        if self.idle_hibernation.pending || self.idle_hibernation.frame.is_some() {
            self.idle_hibernation.last_wake_reason = Some(reason.to_string());
        }
        if self.idle_hibernation.frame.take().is_some() {
            self.idle_hibernation.wake_count += 1;
            if std::mem::take(&mut self.idle_hibernation.transcript_dropped) {
                self.idle_hibernation.history_reload_needed = true;
            }
            crate::logging::info(&format!("TUI hibernation wake: {reason}"));
            self.idle_hibernation.notice.clear();
            // Images and prepared frames were discarded, so repaint all cells
            // once, without an intervening terminal clear/flicker.
            self.force_full_repaint = true;
        }
        self.idle_hibernation.pending = false;
        self.idle_hibernation.measure_after_frame = false;
        self.idle_hibernation.notice_dirty = false;
    }

    /// Explicit test/debug trigger bypasses elapsed time, never the work guard.
    pub(super) fn request_debug_hibernation(&mut self) -> String {
        if self.hibernation_has_work() || self.is_replay {
            return "ERR: cannot hibernate while work is active or queued".to_string();
        }
        if crate::config::config().session.hibernation_idle_secs == 0 {
            return "ERR: session hibernation is disabled".to_string();
        }
        if self.idle_hibernation.frame.is_none() {
            self.idle_hibernation.pending = true;
            self.force_full_repaint = true;
        }
        self.debug_hibernation_state()
    }

    pub(super) fn debug_hibernation_state(&self) -> String {
        serde_json::json!({
            "state": if self.idle_hibernation.frame.is_some() { "hibernated" }
                else if self.idle_hibernation.pending { "pending" } else { "awake" },
            "scope": match crate::config::config().session.hibernation_mode {
                jcode_config_types::HibernationMode::Caches => "tui_derived_caches",
                jcode_config_types::HibernationMode::Transcript => "tui_transcript",
                jcode_config_types::HibernationMode::Exit => "tui_process",
            },
            "source_transcripts_retained": !self.idle_hibernation.transcript_dropped,
            "memory_metric": "pss_anon",
            "busy": self.hibernation_has_work(),
            "wake_count": self.idle_hibernation.wake_count,
            "hibernation_count": self.idle_hibernation.hibernation_count,
            "last_wake_reason": self.idle_hibernation.last_wake_reason,
            "rss_before_bytes": self.idle_hibernation.rss_before,
            "rss_after_bytes": self.idle_hibernation.rss_after,
            "rss_released_bytes": self.idle_hibernation.rss_before.zip(self.idle_hibernation.rss_after)
                .map(|(before, after)| before.saturating_sub(after)),
            "rss_change_bytes": self.idle_hibernation.rss_before.zip(self.idle_hibernation.rss_after)
                .map(|(before, after)| i128::from(after) - i128::from(before)),
            "measurement_phase": if self.idle_hibernation.measure_after_frame { "eviction" } else { "after_frame" },
            "frozen_cells": self.idle_hibernation.frame.as_ref().map(|frame| frame.content.len()),
            "notice": self.idle_hibernation.notice,
        }).to_string()
    }
}

fn frame_can_be_frozen(frame: &Buffer) -> bool {
    !frame.content.is_empty()
        && frame.content.len() <= MAX_FROZEN_CELLS
        && frame
            .content
            .iter()
            .try_fold(0usize, |bytes, cell| bytes.checked_add(cell.symbol().len()))
            .is_some_and(|bytes| bytes <= MAX_FROZEN_SYMBOL_BYTES)
}

/// Memory owned by this process alone. RSS also counts the executable and
/// shared libraries, which every jcode client maps once and shares, so RSS
/// changes overstate (and hide) what hibernation actually frees.
fn private_memory_bytes() -> Option<u64> {
    let snapshot = crate::process_memory::snapshot();
    snapshot
        .os
        .as_ref()
        .and_then(|os| os.pss_anon_bytes)
        .or(snapshot.rss_bytes)
}

fn hibernation_notice(before: Option<u64>, after: Option<u64>) -> String {
    match (before, after) {
        (Some(before), Some(after)) => {
            let change = if before > after {
                format!("-{:.1} MiB", (before - after) as f64 / 1_048_576.0)
            } else if after > before {
                format!("+{:.1} MiB", (after - before) as f64 / 1_048_576.0)
            } else {
                "unchanged".to_string()
            };
            format!(
                "💤 Hibernated · private mem {change} ({:.0} MiB) · type to resume",
                after as f64 / 1_048_576.0,
            )
        }
        _ => "💤 Hibernated · type to resume".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::TuiState;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};

    fn app() -> App {
        crate::tui::app::tests::create_test_app()
    }

    fn expire_idle_timer(app: &mut App) -> Instant {
        let now = Instant::now();
        let old = now - Duration::from_secs(1801);
        app.app_started = old;
        app.idle_hibernation.last_busy = old;
        app.last_user_interaction = Some(old);
        app.last_stream_activity = Some(old);
        now
    }

    fn freeze(app: &mut App) {
        let now = expire_idle_timer(app);
        assert!(app.poll_idle_hibernation_at(now, 1800));
        app.enter_idle_hibernation(Some(&Buffer::empty(Rect::new(0, 0, 100, 30))));
        assert!(app.hibernation_frame().is_some());
    }

    #[test]
    fn idle_hibernation_respects_timeout_typing_streaming_and_disabled_config() {
        let mut app = app();
        let now = expire_idle_timer(&mut app);
        assert!(!app.poll_idle_hibernation_at(now, 0));
        let now = expire_idle_timer(&mut app);
        assert!(!app.poll_idle_hibernation_at(now, 1802));
        app.last_user_interaction = Some(now);
        assert!(!app.poll_idle_hibernation_at(now, 1800));
        app.last_user_interaction = None;
        app.last_stream_activity = Some(now);
        assert!(!app.poll_idle_hibernation_at(now, 1800));
        app.last_stream_activity = None;
        assert!(app.poll_idle_hibernation_at(now, 1800));
    }

    #[test]
    fn idle_hibernation_rechecks_every_queued_and_active_work_guard() {
        let cases: &[fn(&mut App)] = &[
            |app| app.is_processing = true,
            |app| app.pending_turn = true,
            |app| app.pending_queued_dispatch = true,
            |app| app.queued_messages.push("pending".into()),
            |app| app.queued_commands.push("/help".into()),
            |app| app.hidden_queued_system_messages.push("pending".into()),
            |app| app.streaming.streaming_text = "stream".into(),
            |app| app.current_message_id = Some(1),
            |app| app.deferred_stream_done_id = Some(1),
            |app| app.pending_split_request = true,
            |app| app.pending_transfer_request = true,
            |app| app.submit_input_on_startup = true,
            |app| app.remote_model_switch_in_flight = true,
            |app| app.dictation_in_flight = true,
            |app| app.pending_soft_interrupts.push("pending".into()),
            |app| app.session_save_pending = true,
        ];
        for (index, set_busy) in cases.iter().enumerate() {
            let mut app = app();
            let now = expire_idle_timer(&mut app);
            assert!(app.poll_idle_hibernation_at(now, 1800));
            set_busy(&mut app);
            // Work arriving after the timer but before the paint is protected.
            app.enter_idle_hibernation(Some(&Buffer::empty(Rect::new(0, 0, 80, 24))));
            assert!(app.hibernation_frame().is_none(), "guard {index}");
            assert!(!app.poll_idle_hibernation_at(now, 1800), "guard {index}");
        }
    }

    #[test]
    fn idle_hibernation_keeps_sources_draft_scroll_and_first_typed_character() {
        let _render_lock = crate::tui::ui::render_state_test_lock();
        let mut app = app();
        app.push_display_message(DisplayMessage::system("local-only transcript"));
        app.messages.push(Message::user("provider source"));
        app.session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "stored source".into(),
                cache_control: None,
            }],
        );
        app.input = "drft".into();
        app.cursor_pos = 2;
        app.pending_images
            .push(("image/png".into(), "attachment".into()));
        app.pasted_contents.push("expanded paste".into());
        app.scroll_offset = 17;
        app.auto_scroll_paused = true;
        app.typing_scroll_lock = true;
        let session_id = app.session.id.clone();
        let display_count = app.display_messages.len();
        let stored_count = app.session.messages.len();
        freeze(&mut app);
        assert_eq!(app.input, "drft");
        assert_eq!(app.cursor_pos, 2);
        assert_eq!(app.scroll_offset, 17);
        assert!(app.auto_scroll_paused);
        assert_eq!(app.session.id, session_id);
        assert_eq!(app.messages.len(), 1);
        assert_eq!(app.display_messages.len(), display_count);
        assert_eq!(app.session.messages.len(), stored_count);
        assert_eq!(app.pending_images.len(), 1);
        assert_eq!(app.pasted_contents, ["expanded paste"]);
        app.handle_key_event(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert!(app.hibernation_frame().is_none());
        assert_eq!(app.input, "draft");
        assert_eq!(app.cursor_pos, 3);
        assert_eq!(app.scroll_offset, 17);
        assert!(app.hibernation_notice().is_none());
    }

    #[test]
    fn idle_hibernation_paste_resize_and_display_changes_wake_without_loss() {
        let _render_lock = crate::tui::ui::render_state_test_lock();
        let mut app = app();
        freeze(&mut app);
        app.handle_paste("pasted text".into());
        assert!(app.hibernation_frame().is_none());
        assert_eq!(app.input, "pasted text");
        freeze(&mut app);
        app.should_redraw_after_resize();
        assert!(app.hibernation_frame().is_none());
        freeze(&mut app);
        app.push_display_message(DisplayMessage::system("new output"));
        assert!(app.hibernation_frame().is_none());
        assert_eq!(app.display_messages.last().unwrap().content, "new output");
    }

    #[test]
    fn idle_hibernation_frozen_frames_do_not_repopulate_prepared_caches() {
        let _render_lock = crate::tui::ui::render_state_test_lock();
        let mut app = app();
        app.push_display_message(DisplayMessage::system("retained transcript".repeat(300)));
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let frame = terminal
            .draw(|frame| crate::tui::ui::draw(frame, &app))
            .unwrap()
            .buffer
            .clone();
        let prepared = crate::tui::ui::last_chat_frame().unwrap();
        let weak = Arc::downgrade(&prepared);
        drop(prepared);
        let now = expire_idle_timer(&mut app);
        assert!(app.poll_idle_hibernation_at(now, 1800));
        app.enter_idle_hibernation(Some(&frame));
        assert!(
            weak.upgrade().is_none(),
            "all prepared-frame Arc owners must release"
        );
        assert!(crate::tui::ui::idle_render_caches_empty());
        for _ in 0..3 {
            terminal
                .draw(|frame| crate::tui::ui::draw(frame, &app))
                .unwrap();
            assert!(crate::tui::ui::idle_render_caches_empty());
        }
        let screen: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.contains("Hibernated"));
        assert_eq!(
            screen.contains("private mem"),
            app.idle_hibernation.rss_before.is_some() && app.idle_hibernation.rss_after.is_some(),
            "memory details are shown only when both OS samples are available"
        );
        assert!(screen.contains("type to resume"));
        let snapshot = app.hibernation_frame().unwrap().clone();
        app.enter_idle_hibernation(Some(&Buffer::empty(Rect::new(0, 0, 5, 5))));
        assert_eq!(app.hibernation_frame().unwrap(), &snapshot);
        app.wake_idle_hibernation();
        app.wake_idle_hibernation();
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &app))
            .unwrap();
        assert!(!crate::tui::ui::idle_render_caches_empty());
    }

    #[test]
    fn idle_hibernation_rejects_unbounded_and_missing_screen_snapshots() {
        let mut app = app();
        let now = expire_idle_timer(&mut app);
        assert!(app.poll_idle_hibernation_at(now, 1800));
        app.enter_idle_hibernation(None);
        assert!(app.hibernation_frame().is_none());
        let mut frame = Buffer::empty(Rect::new(0, 0, 1, 1));
        frame.content[0].set_symbol(&"x".repeat(MAX_FROZEN_SYMBOL_BYTES + 1));
        assert!(!frame_can_be_frozen(&frame));
        assert!(!frame_can_be_frozen(&Buffer::empty(Rect::new(0, 0, 0, 0))));
    }

    #[test]
    fn private_memory_metric_prefers_pss_anon() {
        if let Some(os) = crate::process_memory::snapshot().os
            && let Some(anon) = os.pss_anon_bytes
        {
            let measured = private_memory_bytes().unwrap();
            // Same metric, sampled moments apart.
            assert!(measured.abs_diff(anon) < 64 << 20);
        }
    }

    #[test]
    fn transcript_drop_marks_history_reload_on_wake_only_when_dropped() {
        let mut app = app();
        freeze(&mut app);
        app.wake_idle_hibernation();
        assert!(!app.idle_hibernation.history_reload_needed);
        freeze(&mut app);
        app.idle_hibernation.transcript_dropped = true;
        app.wake_idle_hibernation();
        assert!(app.idle_hibernation.history_reload_needed);
        assert!(!app.idle_hibernation.transcript_dropped);
    }

    #[test]
    fn idle_hibernation_rss_notice_reports_measurement_without_inventing_savings() {
        let mib = 1_048_576;
        assert!(hibernation_notice(Some(100 * mib), Some(70 * mib)).contains("-30.0 MiB"));
        assert!(hibernation_notice(Some(70 * mib), Some(100 * mib)).contains("+30.0 MiB"));
        assert!(hibernation_notice(Some(70 * mib), Some(70 * mib)).contains("unchanged"));
        for (before, after) in [(None, None), (Some(100), None), (None, Some(100))] {
            let notice = hibernation_notice(before, after);
            assert!(!notice.contains('\n'));
            assert!(!notice.contains("private mem"));
            assert!(notice.contains("type to resume"));
        }
    }

    #[test]
    fn idle_hibernation_ignores_heartbeat_but_wakes_before_remote_stream_event() {
        let _render_lock = crate::tui::ui::render_state_test_lock();
        let mut app = app();
        app.is_remote = true;
        freeze(&mut app);
        let mut remote = crate::tui::backend::ReplayRemoteState::default();
        let pong = serde_json::from_value(serde_json::json!({"type": "pong", "id": 1})).unwrap();
        app.handle_server_event(pong, &mut remote);
        assert!(app.hibernation_frame().is_some());
        app.handle_server_event(crate::protocol::ServerEvent::Ack { id: 2 }, &mut remote);
        assert!(app.hibernation_frame().is_some());
        assert_eq!(app.idle_hibernation.wake_count, 0);
        app.handle_server_event(
            crate::protocol::ServerEvent::TextDelta {
                text: "external work".into(),
            },
            &mut remote,
        );
        assert!(app.hibernation_frame().is_none());
        assert!(app.is_processing);
        assert!(app.hibernation_notice().is_none());
        assert_eq!(app.idle_hibernation.wake_count, 1);
    }

    #[test]
    fn idle_hibernation_debug_request_survives_recent_input_and_read_only_queries() {
        let _render_lock = crate::tui::ui::render_state_test_lock();
        let mut app = app();
        app.note_client_interaction();
        app.idle_hibernation.pending = true;
        let response: serde_json::Value =
            serde_json::from_str(&app.debug_hibernation_state()).unwrap();
        assert_eq!(response["state"], "pending");
        assert!(app.poll_idle_hibernation_at(Instant::now(), 1800));
        app.enter_idle_hibernation(Some(&Buffer::empty(Rect::new(0, 0, 80, 24))));
        for _ in 0..3 {
            let response: serde_json::Value =
                serde_json::from_str(&app.debug_hibernation_state()).unwrap();
            assert_eq!(response["state"], "hibernated");
            assert_eq!(response["hibernation_count"], 1);
            assert_eq!(response["wake_count"], 0);
        }
    }

    #[test]
    fn idle_hibernation_notice_shows_measured_rss_on_one_status_row() {
        let _render_lock = crate::tui::ui::render_state_test_lock();
        let mut app = app();
        let now = expire_idle_timer(&mut app);
        assert!(app.poll_idle_hibernation_at(now, 1800));
        app.enter_idle_hibernation(Some(&Buffer::empty(Rect::new(0, 0, 70, 10))));
        app.idle_hibernation.notice = hibernation_notice(Some(100 << 20), Some(70 << 20));
        let mut terminal = Terminal::new(TestBackend::new(70, 10)).unwrap();
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &app))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Hibernated"));
        assert!(text.contains("private mem -30.0 MiB"));
        assert!(text.contains("resume"));
    }
}
