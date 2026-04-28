use super::state_ui_storage::{
    compact_display_message_tool_data, compact_display_messages_for_storage,
};
use super::*;

const COMPACTED_HISTORY_CHUNK_MESSAGES: usize = 64;
const COMPACTED_HISTORY_LOAD_SCROLL_THRESHOLD: usize = 2;
const COMPACTED_HISTORY_MARKER_PREFIX: &str = "Earlier conversation compacted — ";

impl App {
    pub fn push_display_message(&mut self, mut message: DisplayMessage) {
        compact_display_message_tool_data(&mut message);
        if self.try_coalesce_repeated_display_message(&message) {
            return;
        }
        let is_tool = message.role == "tool";
        self.display_messages.push(message);
        self.bump_display_messages_version();
        if is_tool && self.diff_mode.has_side_pane() && self.diff_pane_auto_scroll {
            self.diff_pane_scroll = usize::MAX;
        }
    }

    pub(super) fn replace_display_messages(&mut self, mut messages: Vec<DisplayMessage>) {
        compact_display_messages_for_storage(&mut messages);
        self.display_messages = messages;
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

        self.replace_display_message_title_and_content(idx, title, content)
    }

    pub(super) fn upsert_background_task_progress_message(&mut self, content: String) {
        let Some(progress) =
            crate::message::parse_background_task_progress_notification_markdown(&content)
        else {
            self.push_display_message(DisplayMessage::background_task(content));
            return;
        };

        let idx = self.display_messages.iter().rposition(|message| {
            message.role == "background_task"
                && crate::message::parse_background_task_progress_notification_markdown(
                    &message.content,
                )
                .is_some_and(|existing| existing.task_id == progress.task_id)
        });

        if let Some(idx) = idx {
            self.replace_display_message_content(idx, content);
        } else {
            self.push_display_message(DisplayMessage::background_task(content));
        }
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
        if !self.display_messages.is_empty() {
            self.display_messages.clear();
            self.bump_display_messages_version();
        }
    }

    pub(super) fn apply_compacted_history_window(
        &mut self,
        mut messages: Vec<DisplayMessage>,
        images: Vec<crate::session::RenderedImage>,
        total_messages: usize,
        visible_messages: usize,
        remaining_messages: usize,
    ) {
        compact_display_messages_for_storage(&mut messages);
        self.display_messages = messages;
        self.remote_side_pane_images = images;
        self.compacted_history_lazy = CompactedHistoryLazyState {
            total_messages,
            visible_messages,
            remaining_messages,
            pending_request_visible: None,
        };
        self.auto_scroll_paused = true;
        self.scroll_offset = 0;
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

    pub(super) fn maybe_queue_compacted_history_load(&mut self) {
        if !self.auto_scroll_paused {
            return;
        }
        if self.scroll_offset > COMPACTED_HISTORY_LOAD_SCROLL_THRESHOLD {
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

        let next_visible = self
            .compacted_history_lazy
            .visible_messages
            .saturating_add(COMPACTED_HISTORY_CHUNK_MESSAGES)
            .min(self.compacted_history_lazy.total_messages);
        if next_visible <= self.compacted_history_lazy.visible_messages {
            return;
        }

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

    pub(super) fn restore_pending_compacted_history_load(&mut self, visible_messages: usize) {
        self.compacted_history_lazy.pending_request_visible = Some(visible_messages);
    }

    #[cfg(test)]
    pub(super) fn compacted_history_lazy_state(&self) -> &CompactedHistoryLazyState {
        &self.compacted_history_lazy
    }

    fn sync_compacted_history_lazy_from_display_messages(&mut self) {
        self.compacted_history_lazy = self
            .display_messages
            .first()
            .and_then(parse_compacted_history_marker)
            .unwrap_or_default();
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
            pending_request_visible: None,
        });
    }

    if after_first.starts_with(" historical messages hidden") {
        return Some(CompactedHistoryLazyState {
            total_messages: first,
            visible_messages: 0,
            remaining_messages: first,
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
