//! Shared TUI state and logic used across TUI runtime paths.
//!
//! This module contains the common display state, input handling,
//! and helper methods used by both local and remote TUI modes.
use super::DisplayMessage;

/// Find the byte offset of the previous character boundary before `pos`.
/// Returns 0 if `pos` is 0 or at the start.
pub fn prev_char_boundary(s: &str, pos: usize) -> usize {
    let mut p = pos;
    if p == 0 {
        return 0;
    }
    p -= 1;
    while p > 0 && !s.is_char_boundary(p) {
        p -= 1;
    }
    p
}

/// Find the byte offset of the next character boundary after `pos`.
/// Returns `s.len()` if already at or past the end.
pub fn next_char_boundary(s: &str, pos: usize) -> usize {
    let mut p = pos + 1;
    while p < s.len() && !s.is_char_boundary(p) {
        p += 1;
    }
    p.min(s.len())
}

/// Convert a byte offset in a string to a character (grapheme) index.
/// Needed when the renderer works in character space but cursor_pos is byte-based.
pub fn byte_offset_to_char_index(s: &str, byte_offset: usize) -> usize {
    s[..byte_offset.min(s.len())].chars().count()
}

/// Convert a character index back to a byte offset.
/// Returns `s.len()` when the requested index is at or beyond the end.
pub fn char_index_to_byte_offset(s: &str, char_index: usize) -> usize {
    if char_index == 0 {
        return 0;
    }

    s.char_indices()
        .nth(char_index)
        .map(|(idx, _)| idx)
        .unwrap_or(s.len())
}

// ========== DisplayMessage Helpers ==========

pub(crate) trait DisplayMessageRoleExt {
    /// Return the role that should be used for rendering.
    ///
    /// Background-task notifications are persisted/injected through a few older
    /// paths that can lose the dedicated `background_task` display role and come
    /// back as plain `user`/`system` markdown. Detect the canonical notification
    /// shape so those messages still render as the rounded background-task card.
    fn effective_role(&self) -> &str;
}

impl DisplayMessageRoleExt for DisplayMessage {
    fn effective_role(&self) -> &str {
        if self.role != "background_task"
            && self.role != "tool"
            && is_background_task_notification_content(&self.content)
        {
            "background_task"
        } else {
            self.role.as_str()
        }
    }
}

fn is_background_task_notification_content(content: &str) -> bool {
    crate::message::parse_background_task_notification_markdown(content).is_some()
        || crate::message::parse_background_task_progress_notification_markdown(content).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_tui_messages::DisplayMessage;

    #[test]
    fn test_display_message_helpers() {
        let msg = DisplayMessage::error("something went wrong");
        assert_eq!(msg.role, "error");
        assert_eq!(msg.content, "something went wrong");

        let msg = DisplayMessage::user("hello").with_title("greeting");
        assert_eq!(msg.role, "user");
        assert_eq!(msg.title, Some("greeting".to_string()));
    }

    #[test]
    fn test_byte_offset_to_char_index() {
        assert_eq!(byte_offset_to_char_index("hello", 0), 0);
        assert_eq!(byte_offset_to_char_index("hello", 3), 3);
        assert_eq!(byte_offset_to_char_index("hello", 5), 5);

        // Korean: each char is 3 bytes
        assert_eq!(byte_offset_to_char_index("한글", 0), 0);
        assert_eq!(byte_offset_to_char_index("한글", 3), 1);
        assert_eq!(byte_offset_to_char_index("한글", 6), 2);

        // Mixed
        assert_eq!(byte_offset_to_char_index("a한b", 0), 0);
        assert_eq!(byte_offset_to_char_index("a한b", 1), 1);
        assert_eq!(byte_offset_to_char_index("a한b", 4), 2);
        assert_eq!(byte_offset_to_char_index("a한b", 5), 3);
    }

    #[test]
    fn test_char_boundary_helpers() {
        let s = "한글test";
        // "한" is bytes 0..3, "글" is bytes 3..6, "test" is bytes 6..10
        assert_eq!(prev_char_boundary(s, 3), 0);
        assert_eq!(prev_char_boundary(s, 6), 3);
        assert_eq!(prev_char_boundary(s, 7), 6);
        assert_eq!(prev_char_boundary(s, 0), 0);

        assert_eq!(next_char_boundary(s, 0), 3);
        assert_eq!(next_char_boundary(s, 3), 6);
        assert_eq!(next_char_boundary(s, 6), 7);
        assert_eq!(next_char_boundary(s, 9), 10);
    }
}
