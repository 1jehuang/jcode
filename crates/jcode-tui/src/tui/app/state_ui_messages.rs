use super::state_ui_storage::{
    compact_display_message_tool_data, compact_display_messages_for_storage,
};
use super::*;
use crate::overnight::OvernightRunStatus;
use std::time::{Duration, Instant};

const COMPACTED_HISTORY_CHUNK_MESSAGES: usize = 64;
const COMPACTED_HISTORY_LOAD_SCROLL_THRESHOLD: usize = 2;
const COMPACTED_HISTORY_MARKER_PREFIX: &str = "Earlier conversation compacted - ";
const OVERNIGHT_CARD_REFRESH_INTERVAL: Duration = Duration::from_secs(5);

fn display_message_from_stored_message(
    message: &crate::session::StoredMessage,
) -> Option<DisplayMessage> {
    let text = stored_message_visible_text(message);
    if text.trim().is_empty() {
        return None;
    }
    if is_background_task_lifecycle_message(&text) {
        return None;
    }
    match message.display_role {
        Some(crate::session::StoredDisplayRole::System) => Some(DisplayMessage::system(text)),
        Some(crate::session::StoredDisplayRole::BackgroundTask) => None,
        None => match message.role {
            Role::User => {
                if crate::session::is_scheduled_task_message(message) {
                    return Some(DisplayMessage::system(text));
                }
                // Synthetic auto-poke continuations are persisted as user
                // turns for the model but must not display as user prompts.
                if crate::todo::is_auto_poke_message(&text) {
                    // Gate continuations are written for the model; the user
                    // only needs to know the check happened.
                    match crate::todo::auto_poke_display_summary(&text) {
                        Some(summary) => Some(DisplayMessage::system(summary.to_string())),
                        None => Some(DisplayMessage::system(text)),
                    }
                } else {
                    Some(DisplayMessage::user(text))
                }
            }
            Role::Assistant => Some(DisplayMessage::assistant(text)),
        },
    }
}

fn is_background_task_lifecycle_message(content: &str) -> bool {
    let content = content.trim_start();
    content.starts_with("**Background task**")
        || content.starts_with("**Background task started**")
        || content.starts_with("**Background task progress**")
        || content.starts_with("**Background task stalled**")
}

fn stored_message_visible_text(message: &crate::session::StoredMessage) -> String {
    let mut parts = Vec::new();
    for block in &message.content {
        match block {
            ContentBlock::Text { text, .. }
            | ContentBlock::Reasoning { text }
            | ContentBlock::ReasoningTrace { text } => {
                if !text.trim().is_empty() {
                    parts.push(text.trim().to_string());
                }
            }
            ContentBlock::AnthropicThinking { .. } | ContentBlock::OpenAIReasoning { .. } => {}
            ContentBlock::ToolUse { name, input, .. } => {
                parts.push(format!("[tool:{} {}]", name, input));
            }
            ContentBlock::ToolResult { content, .. } => {
                if !content.trim().is_empty() {
                    parts.push(content.trim().to_string());
                }
            }
            ContentBlock::Image { media_type, .. } => {
                parts.push(format!("[image:{}]", media_type));
            }
            ContentBlock::OpenAICompaction { .. } => {}
        }
    }
    parts.join("\n\n")
}

impl App {
    pub fn push_display_message(&mut self, mut message: DisplayMessage) {
        if is_background_task_lifecycle_message(&message.content) {
            return;
        }
        compact_display_message_tool_data(&mut message);
        // A trailing Ctrl+L spacer only exists to keep the screen clear while
        // idle. The moment real content arrives, drop it so the transcript
        // stays contiguous instead of keeping a screenful of blank rows
        // embedded in the scrollback.
        if message.role != "spacer"
            && self
                .display_messages
                .last()
                .is_some_and(|last| last.role == "spacer")
        {
            self.display_messages.pop();
        }
        if self.try_coalesce_repeated_display_message(&message) {
            return;
        }
        let is_tool = message.role == "tool";
        // Track the trailing run of assistant messages so a provider
        // RetryRollback can remove exactly the current attempt's committed
        // output. Any non-assistant message (user/tool/system) is a fence: it
        // proves earlier assistant messages belong to completed work.
        if message.role == "assistant" {
            self.attempt_committed_assistant_messages += 1;
        } else {
            self.attempt_committed_assistant_messages = 0;
        }
        // Maintain the cached display-message counters incrementally for this
        // single append, then bump the version without a full O(M) rescan.
        // Appending is the hot path; rescanning every append was O(M^2) over a
        // long session.
        self.adjust_display_message_stats(&message, true);
        self.display_messages.push(message);
        self.bump_display_messages_version_no_stats();
        if is_tool && self.diff_mode.has_side_pane() && self.diff_pane_auto_scroll {
            self.diff_pane_scroll = usize::MAX;
        }
    }

    pub(super) fn replace_display_messages(&mut self, mut messages: Vec<DisplayMessage>) {
        messages.retain(|message| !is_background_task_lifecycle_message(&message.content));
        compact_display_messages_for_storage(&mut messages);
        self.display_messages = messages;
        self.attempt_committed_assistant_messages = 0;
        self.sync_compacted_history_lazy_from_display_messages();
        self.bump_display_messages_version();
        self.note_runtime_memory_event_force("display_messages_replaced", "display_history_reset");
    }

    pub(super) fn replace_display_message_content(&mut self, idx: usize, content: String) -> bool {
        if let Some(message) = self.display_messages.get_mut(idx) {
            if message.content != content {
                message.content = content;
                self.bump_display_messages_version();
            }
            true
        } else {
            false
        }
    }

    pub(super) fn replace_display_message_title_and_content(
        &mut self,
        idx: usize,
        title: Option<String>,
        content: String,
    ) -> bool {
        if let Some(message) = self.display_messages.get_mut(idx) {
            if message.title != title || message.content != content {
                message.title = title;
                message.content = content;
                self.bump_display_messages_version();
            }
            true
        } else {
            false
        }
    }

    pub(super) fn replace_latest_tool_display_message(
        &mut self,
        tool_call_id: &str,
        title: Option<String>,
        content: String,
    ) -> bool {
        let Some(idx) = self.display_messages.iter().rposition(|message| {
            message.tool_data.as_ref().map(|tool| tool.id.as_str()) == Some(tool_call_id)
        }) else {
            return false;
        };

        // A tool moved to the background finishes its foreground card with the
        // background lifecycle notification returned by the tool. The same
        // notification also drives the retained row in the pinned status band,
        // so remove the transient tool card instead of turning it into a second
        // transcript representation.
        if is_background_task_lifecycle_message(&content) {
            self.remove_display_message(idx);
            return true;
        }

        self.replace_display_message_title_and_content(idx, title, content)
    }

    pub(super) fn background_task_rows_ref(&self) -> &[crate::tui::BackgroundTaskRow] {
        &self.background_task_rows
    }

    fn retain_latest_background_tasks(&mut self) {
        const MAX_PINNED_BACKGROUND_TASKS: usize = 2;
        if self.background_task_rows.len() > MAX_PINNED_BACKGROUND_TASKS {
            let stale = self.background_task_rows.len() - MAX_PINNED_BACKGROUND_TASKS;
            self.background_task_rows.drain(..stale);
        }
    }

    pub(super) fn upsert_running_background_task(
        &mut self,
        task_id: String,
        label: String,
        percent: Option<f32>,
    ) {
        if let Some(index) = self
            .background_task_rows
            .iter()
            .position(|task| task.task_id == task_id)
        {
            let mut task = self.background_task_rows.remove(index);
            task.label = label;
            // Output parsers can alternate between determinate updates (for
            // example `42%`) and phase-only updates (`Compiling foo`).  A
            // phase-only update must not erase the last useful percentage,
            // otherwise the pinned progress bar jumps back to zero until the
            // task completes at 100%.
            if percent.is_some() || task.percent.is_none() {
                task.percent = percent;
            }
            task.status = crate::tui::BackgroundTaskRowStatus::Running;
            task.completed_at = None;
            self.background_task_rows.push(task);
            return;
        }
        self.background_task_rows
            .push(crate::tui::BackgroundTaskRow {
                task_id,
                label,
                percent,
                status: crate::tui::BackgroundTaskRowStatus::Running,
                completed_at: None,
            });
        self.retain_latest_background_tasks();
    }

    pub(super) fn upsert_running_background_task_progress(&mut self, content: &str) -> bool {
        let Some(progress) =
            crate::message::parse_background_task_progress_notification_markdown(content)
        else {
            return false;
        };
        let label = crate::message::background_task_display_label(
            &progress.tool_name,
            progress.display_name.as_deref(),
        );
        self.upsert_running_background_task(progress.task_id, label, progress.percent);
        true
    }

    pub(super) fn upsert_running_background_task_started(&mut self, content: &str) -> bool {
        let Some(started) =
            crate::message::parse_background_task_started_notification_markdown(content)
        else {
            return false;
        };
        self.upsert_running_background_task(started.task_id, started.label, None);
        true
    }

    pub(super) fn finish_background_task(
        &mut self,
        task_id: String,
        label: String,
        status: crate::tui::BackgroundTaskRowStatus,
    ) {
        if let Some(index) = self
            .background_task_rows
            .iter()
            .position(|task| task.task_id == task_id)
        {
            let mut task = self.background_task_rows.remove(index);
            task.label = label;
            task.status = status;
            task.completed_at = (status == crate::tui::BackgroundTaskRowStatus::Completed)
                .then(std::time::Instant::now);
            if status == crate::tui::BackgroundTaskRowStatus::Completed {
                task.percent = Some(100.0);
            }
            self.background_task_rows.push(task);
            return;
        }
        self.background_task_rows
            .push(crate::tui::BackgroundTaskRow {
                task_id,
                label,
                percent: (status == crate::tui::BackgroundTaskRowStatus::Completed)
                    .then_some(100.0),
                status,
                completed_at: (status == crate::tui::BackgroundTaskRowStatus::Completed)
                    .then(std::time::Instant::now),
            });
        self.retain_latest_background_tasks();
    }

    /// Successful tasks are useful as short-lived confirmation, but should not
    /// permanently consume the pinned todo band's limited space. Failures stay
    /// until acted on, and running tasks always stay visible.
    pub(super) fn prune_irrelevant_background_tasks(&mut self) -> bool {
        const COMPLETED_TASK_VISIBILITY: std::time::Duration = std::time::Duration::from_secs(12);
        let now = std::time::Instant::now();
        let previous_len = self.background_task_rows.len();
        self.background_task_rows.retain(|task| {
            task.completed_at.is_none_or(|completed_at| {
                now.saturating_duration_since(completed_at) < COMPLETED_TASK_VISIBILITY
            })
        });
        self.background_task_rows.len() != previous_len
    }

    pub(super) fn upsert_overnight_display_card(
        &mut self,
        manifest: &crate::overnight::OvernightManifest,
    ) -> bool {
        let Ok(content) = crate::overnight::format_progress_card_content(manifest) else {
            return false;
        };
        let title = Some("Overnight".to_string());
        let idx = self.display_messages.iter().rposition(|message| {
            message.role == "overnight"
                && serde_json::from_str::<crate::overnight::OvernightProgressCard>(&message.content)
                    .is_ok_and(|card| card.run_id == manifest.run_id)
        });
        if let Some(idx) = idx {
            self.replace_display_message_title_and_content(idx, title, content)
        } else {
            self.push_display_message(DisplayMessage::overnight(content));
            true
        }
    }

    pub(super) fn maybe_refresh_overnight_display_card(&mut self) -> bool {
        let now = Instant::now();
        if self
            .last_overnight_card_refresh
            .is_some_and(|last| now.duration_since(last) < OVERNIGHT_CARD_REFRESH_INTERVAL)
        {
            return false;
        }
        self.last_overnight_card_refresh = Some(now);

        let has_card = self
            .display_messages
            .iter()
            .any(|message| message.role == "overnight");
        let Ok(Some(manifest)) = crate::overnight::latest_manifest() else {
            return false;
        };
        let active = matches!(
            manifest.status,
            OvernightRunStatus::Running | OvernightRunStatus::CancelRequested
        );
        if !has_card && !active {
            return false;
        }
        let card_changed = self.upsert_overnight_display_card(&manifest);
        let transcript_changed = self.maybe_tail_overnight_current_session_transcript(&manifest);
        card_changed || transcript_changed
    }

    fn maybe_tail_overnight_current_session_transcript(
        &mut self,
        manifest: &crate::overnight::OvernightManifest,
    ) -> bool {
        if manifest.coordinator_session_id != self.session.id {
            return false;
        }
        let Ok(latest_session) = crate::session::Session::load(&self.session.id) else {
            return false;
        };
        if latest_session.messages.len() <= self.session.messages.len() {
            return false;
        }

        let appended: Vec<DisplayMessage> = latest_session.messages[self.session.messages.len()..]
            .iter()
            .filter_map(display_message_from_stored_message)
            .collect();
        self.session = latest_session;
        if appended.is_empty() {
            return false;
        }
        for message in appended {
            self.push_display_message(message);
        }
        true
    }

    pub(super) fn remove_display_message(&mut self, idx: usize) -> Option<DisplayMessage> {
        if idx < self.display_messages.len() {
            let removed = self.display_messages.remove(idx);
            self.bump_display_messages_version();
            Some(removed)
        } else {
            None
        }
    }

    pub(super) fn append_reload_message(&mut self, line: &str) {
        if let Some(idx) = self
            .display_messages
            .iter()
            .rposition(Self::is_reload_message)
        {
            let msg = &mut self.display_messages[idx];
            if !msg.content.is_empty() {
                msg.content.push('\n');
            }
            msg.content.push_str(line);
            msg.title = Some("Reload".to_string());
            self.bump_display_messages_version();
        } else {
            self.push_display_message(
                DisplayMessage::system(line.to_string()).with_title("Reload"),
            );
        }
    }

    pub(super) fn is_client_maintenance_message(message: &DisplayMessage, title: &str) -> bool {
        message.role == "system" && message.title.as_deref() == Some(title)
    }

    pub(super) fn is_reload_message(message: &DisplayMessage) -> bool {
        message.role == "system"
            && message
                .title
                .as_deref()
                .is_some_and(|title| title == "Reload" || title.starts_with("Reload: "))
    }

    fn try_coalesce_repeated_display_message(&mut self, message: &DisplayMessage) -> bool {
        if !Self::is_repeat_compactable_display_message(message) {
            return false;
        }

        let Some(last) = self.display_messages.last_mut() else {
            return false;
        };
        if !Self::is_repeat_compactable_display_message(last) {
            return false;
        }

        let (last_base, last_count) = Self::split_repeat_suffix(&last.content);
        if last.role != message.role
            || last.title != message.title
            || last.tool_calls != message.tool_calls
            || last.duration_secs != message.duration_secs
            || last_base != message.content
        {
            return false;
        }

        let next_count = last_count.saturating_add(1);
        last.content = Self::format_repeated_display_content(message.content.as_str(), next_count);
        self.bump_display_messages_version();
        true
    }

    fn is_repeat_compactable_display_message(message: &DisplayMessage) -> bool {
        matches!(message.role.as_str(), "system" | "error")
            && message.title.is_none()
            && message.tool_calls.is_empty()
            && message.tool_data.is_none()
            && message.duration_secs.is_none()
            && !message.content.contains(['\n', '\r'])
    }

    fn split_repeat_suffix(content: &str) -> (&str, u32) {
        const REPEAT_PREFIX: &str = " [×";

        let Some(prefix_idx) = content.rfind(REPEAT_PREFIX) else {
            return (content, 1);
        };
        if !content.ends_with(']') {
            return (content, 1);
        }

        let digits = &content[prefix_idx + REPEAT_PREFIX.len()..content.len() - 1];
        if digits.is_empty() || !digits.chars().all(|ch| ch.is_ascii_digit()) {
            return (content, 1);
        }

        match digits.parse::<u32>() {
            Ok(count) if count >= 2 => (&content[..prefix_idx], count),
            _ => (content, 1),
        }
    }

    fn format_repeated_display_content(content: &str, repeat_count: u32) -> String {
        if repeat_count <= 1 {
            content.to_string()
        } else {
            format!("{content} [×{repeat_count}]")
        }
    }

    pub(super) fn clear_display_messages(&mut self) {
        self.compacted_history_lazy = CompactedHistoryLazyState::default();
        // The transcript is about to be discarded; forget where the live reasoning
        // block started so a stale offset can't slice the new stream.
        self.reasoning_block_start = None;
        self.turn_reasoning_traces.clear();
        if !self.display_messages.is_empty() {
            self.display_messages.clear();
            self.bump_display_messages_version();
        }
    }

    /// Terminal-style clear (Ctrl+L): append a viewport-height blank spacer
    /// and snap to the bottom, so the screen shows a clean prompt while the
    /// whole transcript stays one scroll-up away, exactly like a terminal's
    /// clear-with-scrollback. Nothing is deleted; context, queue, and draft
    /// are untouched. Contrast with `/cls` (`clear_view_keep_context`), which
    /// actually wipes the rendered transcript.
    pub(super) fn clear_view_terminal_style(&mut self) {
        let rows = super::super::ui::last_chat_viewport_height();
        // Pressing Ctrl+L repeatedly (or on an already-empty screen) should
        // not stack blank pages: with a trailing spacer and nothing after it,
        // the viewport is already visually clear, so just re-snap.
        let already_clear = self
            .display_messages
            .last()
            .is_some_and(|message| message.role == "spacer");
        if rows > 0 && !already_clear && !self.display_messages.is_empty() {
            self.push_display_message(DisplayMessage::spacer(rows));
        }
        self.follow_chat_bottom();
    }

    /// Whether the view is in the terminal-style cleared state: the transcript
    /// ends in a Ctrl+L spacer, the viewport is pinned to the bottom, and no
    /// new output has arrived since. In that state every visible transcript row
    /// is blank, so the renderer collapses the messages area and the numbered
    /// prompt sits at the top of the screen like a terminal after `clear`.
    /// Any new message, stream, or scroll-up immediately ends it.
    pub(crate) fn terminal_clear_collapsed(&self) -> bool {
        !self.auto_scroll_paused
            && self.pending_history_anchor.is_none()
            && !self.is_processing
            && self.streaming.streaming_text.is_empty()
            && self
                .display_messages
                .last()
                .is_some_and(|message| message.role == "spacer")
    }

    /// View-only clear (`/cls`): wipe the rendered transcript while
    /// keeping provider context, queued messages, and the input draft intact,
    /// so the model still remembers everything. Contrast with `/clear`
    /// (`reset_current_session`), which discards context too, and Ctrl+L,
    /// which merely snaps to the bottom of the chat.
    pub(super) fn clear_view_keep_context(&mut self) {
        self.clear_display_messages();
        // The rendered transcript is gone, so every entry in the
        // process-global ACTIVE_DIAGRAMS registry is orphaned (same rationale
        // as reset_current_session; partial-retention paths like /rewind must
        // NOT do this).
        crate::tui::mermaid::clear_active_diagrams();
        self.scroll_offset = 0;
        self.auto_scroll_paused = false;
        self.set_status_notice("View cleared (context kept)");
    }

    pub(super) fn apply_compacted_history_window(
        &mut self,
        mut messages: Vec<DisplayMessage>,
        images: Vec<crate::session::RenderedImage>,
        total_messages: usize,
        visible_messages: usize,
        remaining_messages: usize,
        hidden_user_prompts: usize,
    ) {
        compact_display_messages_for_storage(&mut messages);
        self.display_messages = messages;
        self.remote_side_pane_images = images;
        self.invalidate_side_pane_images_signature();
        self.compacted_history_lazy = CompactedHistoryLazyState {
            total_messages,
            visible_messages,
            remaining_messages,
            hidden_user_prompts,
            pending_request_visible: None,
        };
        self.auto_scroll_paused = true;
        // Older messages are prepended above the current view. If the reader had
        // an anchor captured (they scrolled up to trigger this load), leave the
        // scroll position for the next render to resolve so the content under
        // them stays put instead of teleporting to the new absolute top. Only
        // fall back to the top when there is no anchor to honor.
        if self.pending_history_anchor.is_none() {
            self.scroll_offset = 0;
        }
        self.bump_display_messages_version();
        self.note_runtime_memory_event_force(
            "compacted_history_loaded",
            "display_history_lazy_window",
        );
        if remaining_messages > 0 {
            self.set_status_notice(format!(
                "Loaded {} compacted messages · {} older hidden",
                visible_messages, remaining_messages
            ));
        } else if total_messages > 0 {
            self.set_status_notice(format!("Loaded all {} compacted messages", total_messages));
        }
    }

    /// Number of wrapped lines from the top of the chat viewport that should be
    /// treated as the "near the top" zone that proactively loads older history.
    /// Prefetching roughly one viewport ahead means scrolling up keeps flowing
    /// instead of stalling at a hard wall and then jumping.
    fn compacted_history_prefetch_threshold(&self) -> usize {
        let viewport = crate::tui::ui::last_layout_snapshot()
            .map(|layout| layout.messages_area.height as usize)
            .unwrap_or(0);
        // Trigger when within ~one viewport of the top, with a small floor so the
        // behavior is sensible even before the first layout snapshot exists.
        viewport.max(COMPACTED_HISTORY_LOAD_SCROLL_THRESHOLD)
    }

    /// Capture a viewport anchor describing the reader's current distance from
    /// the bottom of the transcript, plus any leftover upward scroll intent that
    /// could not be satisfied because the view was already at the top of the
    /// currently-loaded content. The next render that includes the newly loaded
    /// (prepended) history resolves this back into an absolute `scroll_offset`,
    /// keeping the content under the reader stable across the load.
    pub(super) fn capture_history_anchor(&mut self, overshoot: usize) {
        // Don't clobber an anchor that is still waiting to be resolved; the
        // original distance-from-bottom remains correct across further prepends.
        if self.pending_history_anchor.is_some() {
            return;
        }
        let total = crate::tui::ui::last_total_wrapped_lines();
        if total == 0 {
            return;
        }
        // The top of the viewport currently sits at absolute line `scroll_offset`
        // within the pre-prepend transcript (length `total`). Its distance from
        // the bottom is invariant when older lines are prepended, so capture it
        // (plus any unsatisfied upward intent as `overshoot`) and let the next
        // render map it back to an absolute offset against the larger total.
        let scroll = self.scroll_offset.min(total);
        let lines_from_bottom = total.saturating_sub(scroll).saturating_add(overshoot);
        self.pending_history_anchor = Some(super::HistoryScrollAnchor {
            lines_from_bottom,
            base_total: total,
        });
    }

    /// Adopt a resolved history anchor once a frame containing the newly loaded
    /// content has rendered. Returns true when the scroll position changed.
    pub(super) fn reconcile_history_anchor(&mut self) -> bool {
        let Some(anchor) = self.pending_history_anchor else {
            return false;
        };
        let total = crate::tui::ui::last_total_wrapped_lines();
        // Wait until a frame with the prepended content has actually rendered
        // (its total wrapped-line count differs from the captured base).
        if total == 0 || total == anchor.base_total {
            return false;
        }
        let resolved = crate::tui::ui::last_resolved_chat_scroll();
        self.pending_history_anchor = None;
        let changed = self.scroll_offset != resolved || !self.auto_scroll_paused;
        self.scroll_offset = resolved;
        self.auto_scroll_paused = true;
        changed
    }

    pub(super) fn maybe_queue_compacted_history_load(&mut self) {
        self.maybe_queue_compacted_history_load_with_overshoot(0);
    }

    pub(super) fn maybe_queue_compacted_history_load_with_overshoot(&mut self, overshoot: usize) {
        if !self.auto_scroll_paused {
            return;
        }
        if self.scroll_offset > self.compacted_history_prefetch_threshold() {
            return;
        }
        if self.compacted_history_lazy.remaining_messages == 0 {
            return;
        }
        if self
            .compacted_history_lazy
            .pending_request_visible
            .is_some()
        {
            return;
        }
        // Throttle to one chunk per settled frame: while an anchor is still
        // waiting to resolve on screen, hold off so prepends never compound into
        // a visible jump.
        if self.pending_history_anchor.is_some() {
            return;
        }

        let next_visible = self
            .compacted_history_lazy
            .visible_messages
            .saturating_add(COMPACTED_HISTORY_CHUNK_MESSAGES)
            .min(self.compacted_history_lazy.total_messages);
        if next_visible <= self.compacted_history_lazy.visible_messages {
            return;
        }

        // Anchor the viewport before mutating so the prepend stays seamless.
        self.capture_history_anchor(overshoot);

        if self.is_remote {
            self.compacted_history_lazy.pending_request_visible = Some(next_visible);
            self.set_status_notice(format!(
                "Loading older compacted history… {} of {}",
                next_visible, self.compacted_history_lazy.total_messages
            ));
        } else {
            self.apply_local_compacted_history_window(next_visible);
        }
    }

    pub(super) fn take_pending_compacted_history_load(&mut self) -> Option<usize> {
        self.compacted_history_lazy.pending_request_visible.take()
    }

    /// Whether there are older compacted-history messages not yet loaded into the
    /// display transcript.
    pub(super) fn compacted_history_has_remaining(&self) -> bool {
        self.compacted_history_lazy.remaining_messages > 0
    }

    pub(super) fn restore_pending_compacted_history_load(&mut self, visible_messages: usize) {
        self.compacted_history_lazy.pending_request_visible = Some(visible_messages);
    }

    #[cfg(test)]
    pub(super) fn compacted_history_lazy_state(&self) -> &CompactedHistoryLazyState {
        &self.compacted_history_lazy
    }

    fn sync_compacted_history_lazy_from_display_messages(&mut self) {
        let mut lazy = self
            .display_messages
            .first()
            .and_then(parse_compacted_history_marker)
            .unwrap_or_default();
        // The marker text does not encode how many prompts are hidden, so derive
        // it from the session render info when a compacted window is in effect.
        // This keeps prompt numbering absolute (the first visible prompt keeps
        // its real turn number).
        if lazy.remaining_messages > 0 || lazy.total_messages > 0 {
            let visible = if lazy.remaining_messages == 0 {
                usize::MAX
            } else {
                lazy.visible_messages
            };
            let (_, _, compacted_info) =
                crate::session::render_messages_and_images_with_compacted_history(
                    &self.session,
                    visible,
                );
            if let Some(info) = compacted_info {
                lazy.hidden_user_prompts = info.hidden_user_prompts;
            }
        }
        self.compacted_history_lazy = lazy;
    }

    fn apply_local_compacted_history_window(&mut self, visible_messages: usize) {
        let (rendered_messages, images, compacted_info) =
            crate::session::render_messages_and_images_with_compacted_history(
                &self.session,
                visible_messages,
            );
        let Some(compacted_info) = compacted_info else {
            return;
        };
        let display_messages = rendered_messages
            .into_iter()
            .map(|msg| DisplayMessage {
                role: msg.role,
                content: msg.content,
                tool_calls: msg.tool_calls,
                duration_secs: None,
                title: None,
                tool_data: msg.tool_data,
            })
            .collect();
        self.apply_compacted_history_window(
            display_messages,
            images,
            compacted_info.total_messages,
            compacted_info.visible_messages,
            compacted_info.remaining_messages,
            compacted_info.hidden_user_prompts,
        );
    }
}

fn parse_compacted_history_marker(message: &DisplayMessage) -> Option<CompactedHistoryLazyState> {
    if message.role != "system" {
        return None;
    }
    let rest = message
        .content
        .strip_prefix(COMPACTED_HISTORY_MARKER_PREFIX)?;

    if let Some(rest) = rest.strip_prefix("showing all ") {
        let (total, _) = parse_leading_usize(rest)?;
        return Some(CompactedHistoryLazyState {
            total_messages: total,
            visible_messages: total,
            remaining_messages: 0,
            hidden_user_prompts: 0,
            pending_request_visible: None,
        });
    }

    let (first, after_first) = parse_leading_usize(rest)?;
    if after_first.starts_with(" older historical messages hidden. Showing ") {
        let showing = after_first.strip_prefix(" older historical messages hidden. Showing ")?;
        let (visible, after_visible) = parse_leading_usize(showing)?;
        let after_visible = after_visible.strip_prefix(" of ")?;
        let (total, _) = parse_leading_usize(after_visible)?;
        return Some(CompactedHistoryLazyState {
            total_messages: total,
            visible_messages: visible,
            remaining_messages: first,
            hidden_user_prompts: 0,
            pending_request_visible: None,
        });
    }

    if after_first.starts_with(" historical messages hidden") {
        return Some(CompactedHistoryLazyState {
            total_messages: first,
            visible_messages: 0,
            remaining_messages: first,
            hidden_user_prompts: 0,
            pending_request_visible: None,
        });
    }

    None
}

fn parse_leading_usize(text: &str) -> Option<(usize, &str)> {
    let end = text
        .char_indices()
        .take_while(|(_, ch)| ch.is_ascii_digit())
        .map(|(idx, ch)| idx + ch.len_utf8())
        .last()?;
    let value = text[..end].parse().ok()?;
    Some((value, &text[end..]))
}
