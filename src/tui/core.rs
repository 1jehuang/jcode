//! Shared TUI state and logic between standalone App and ClientApp
//!
//! This module contains the common display state, input handling,
//! and helper methods used by both local and remote TUI modes.

use super::markdown::IncrementalMarkdownRenderer;
use super::{DisplayMessage, ProcessingStatus};
use crate::message::ToolCall;
use crossterm::event::KeyCode;
use std::time::Instant;

/// Shared TUI state for display and input handling
///
/// This struct contains all the fields that are identical between
/// the standalone App and the remote ClientApp.
pub struct TuiCore {
    // Display state
    pub display_messages: Vec<DisplayMessage>,
    pub streaming_text: String,
    pub streaming_tool_calls: Vec<ToolCall>,
    pub streaming_input_tokens: u64,
    pub streaming_output_tokens: u64,

    // Input state
    pub input: String,
    pub cursor_pos: usize,
    pub pasted_contents: Vec<String>,
    pub queued_messages: Vec<String>,

    // UI state
    pub scroll_offset: usize,
    pub status: ProcessingStatus,
    pub subagent_status: Option<String>,
    /// When true, don't auto-scroll to bottom during streaming
    /// Set when user scrolls up during processing, cleared on new message
    pub auto_scroll_paused: bool,

    // Processing state
    pub is_processing: bool,
    pub should_quit: bool,
    pub processing_started: Option<Instant>,
    pub last_stream_activity: Option<Instant>,

    // Incremental markdown rendering for streaming text
    pub streaming_md_renderer: IncrementalMarkdownRenderer,
}

impl Default for TuiCore {
    fn default() -> Self {
        Self::new()
    }
}

impl TuiCore {
    pub fn new() -> Self {
        Self {
            display_messages: Vec::new(),
            streaming_text: String::new(),
            streaming_tool_calls: Vec::new(),
            streaming_input_tokens: 0,
            streaming_output_tokens: 0,
            input: String::new(),
            cursor_pos: 0,
            pasted_contents: Vec::new(),
            queued_messages: Vec::new(),
            scroll_offset: 0,
            status: ProcessingStatus::Idle,
            subagent_status: None,
            auto_scroll_paused: false,
            is_processing: false,
            should_quit: false,
            processing_started: None,
            last_stream_activity: None,
            streaming_md_renderer: IncrementalMarkdownRenderer::new(None),
        }
    }

    // ========== Input Editing ==========

    /// Insert a character at cursor position
    pub fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor_pos, c);
        self.cursor_pos += 1;
    }

    /// Delete character before cursor (backspace)
    pub fn backspace(&mut self) {
        if self.cursor_pos > 0 {
            self.cursor_pos -= 1;
            self.input.remove(self.cursor_pos);
        }
    }

    /// Delete character at cursor (delete key)
    pub fn delete(&mut self) {
        if self.cursor_pos < self.input.len() {
            self.input.remove(self.cursor_pos);
        }
    }

    /// Move cursor left
    pub fn cursor_left(&mut self) {
        if self.cursor_pos > 0 {
            self.cursor_pos -= 1;
        }
    }

    /// Move cursor right
    pub fn cursor_right(&mut self) {
        if self.cursor_pos < self.input.len() {
            self.cursor_pos += 1;
        }
    }

    /// Move cursor to start of input
    pub fn cursor_home(&mut self) {
        self.cursor_pos = 0;
    }

    /// Move cursor to end of input
    pub fn cursor_end(&mut self) {
        self.cursor_pos = self.input.len();
    }

    /// Clear input and reset cursor
    pub fn clear_input(&mut self) {
        self.input.clear();
        self.cursor_pos = 0;
    }

    /// Clear input from cursor to end (Ctrl+K)
    pub fn kill_to_end(&mut self) {
        self.input.truncate(self.cursor_pos);
    }

    /// Clear entire input line (Ctrl+U)
    pub fn kill_line(&mut self) {
        self.input.clear();
        self.cursor_pos = 0;
    }

    /// Handle basic editing key, returns true if handled
    pub fn handle_edit_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Char(c) => {
                self.insert_char(c);
                true
            }
            KeyCode::Backspace => {
                self.backspace();
                true
            }
            KeyCode::Delete => {
                self.delete();
                true
            }
            KeyCode::Left => {
                self.cursor_left();
                true
            }
            KeyCode::Right => {
                self.cursor_right();
                true
            }
            KeyCode::Home => {
                self.cursor_home();
                true
            }
            KeyCode::End => {
                self.cursor_end();
                true
            }
            _ => false,
        }
    }

    // ========== Scrolling ==========

    /// Scroll up by given amount
    /// If processing, this pauses auto-scroll to let user review content
    pub fn scroll_up(&mut self, amount: usize) {
        // Use generous estimate - UI will clamp to actual content
        let max_estimate = self.display_messages.len() * 100 + self.streaming_text.len();
        self.scroll_offset = (self.scroll_offset + amount).min(max_estimate);
        // Pause auto-scroll when user scrolls up during streaming
        if self.is_processing {
            self.auto_scroll_paused = true;
        }
    }

    /// Scroll down by given amount
    pub fn scroll_down(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
        // If scrolled back to bottom, resume auto-scroll
        if self.scroll_offset == 0 {
            self.auto_scroll_paused = false;
        }
    }

    /// Reset scroll to bottom and resume auto-scroll
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
        self.auto_scroll_paused = false;
    }

    /// Resume auto-scroll (call when new message is submitted)
    pub fn resume_auto_scroll(&mut self) {
        self.auto_scroll_paused = false;
        self.scroll_offset = 0;
    }

    /// Handle scroll key, returns true if handled
    pub fn handle_scroll_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Up => {
                self.scroll_up(1);
                true
            }
            KeyCode::Down => {
                self.scroll_down(1);
                true
            }
            KeyCode::PageUp => {
                self.scroll_up(10);
                true
            }
            KeyCode::PageDown => {
                self.scroll_down(10);
                true
            }
            _ => false,
        }
    }

    // ========== Paste Handling ==========

    /// Handle paste: store content and insert placeholder (or inline for small pastes)
    pub fn handle_paste(&mut self, text: String) {
        let line_count = text.lines().count().max(1);
        if line_count < 5 {
            // Small paste: insert text directly (no placeholder needed)
            self.input.insert_str(self.cursor_pos, &text);
            self.cursor_pos += text.len();
        } else {
            // Large paste: use placeholder
            self.pasted_contents.push(text);
            let placeholder = format!(
                "[pasted {} line{}]",
                line_count,
                if line_count == 1 { "" } else { "s" }
            );
            self.input.insert_str(self.cursor_pos, &placeholder);
            self.cursor_pos += placeholder.len();
        }
    }

    /// Expand paste placeholders in input with actual content
    pub fn expand_paste_placeholders(&mut self, input: &str) -> String {
        let mut result = input.to_string();
        // Replace placeholders in reverse order to preserve indices
        for content in self.pasted_contents.iter().rev() {
            let line_count = content.lines().count().max(1);
            let placeholder = format!(
                "[pasted {} line{}]",
                line_count,
                if line_count == 1 { "" } else { "s" }
            );
            // Use rfind to match last occurrence (since we iterate in reverse)
            if let Some(pos) = result.rfind(&placeholder) {
                result.replace_range(pos..pos + placeholder.len(), content);
            }
        }
        result
    }

    /// Take input, expand placeholders, and clear paste storage
    pub fn take_expanded_input(&mut self) -> (String, String) {
        let raw_input = std::mem::take(&mut self.input);
        let expanded = self.expand_paste_placeholders(&raw_input);
        self.pasted_contents.clear();
        self.cursor_pos = 0;
        (raw_input, expanded)
    }

    // ========== Streaming State ==========

    /// Start processing a new request
    pub fn start_processing(&mut self) {
        self.is_processing = true;
        self.processing_started = Some(Instant::now());
        self.status = ProcessingStatus::Sending;
        self.scroll_offset = 0;
    }

    /// Reset streaming state after completion
    pub fn reset_streaming(&mut self) {
        self.streaming_text.clear();
        self.streaming_tool_calls.clear();
        self.streaming_input_tokens = 0;
        self.streaming_output_tokens = 0;
        self.is_processing = false;
        self.processing_started = None;
        self.status = ProcessingStatus::Idle;
        self.subagent_status = None;
        self.last_stream_activity = None;
        self.streaming_md_renderer.reset();
        crate::tui::mermaid::clear_streaming_preview_diagram();
    }

    /// Update last activity timestamp
    pub fn touch_activity(&mut self) {
        self.last_stream_activity = Some(Instant::now());
    }

    /// Append text to streaming buffer
    pub fn append_streaming_text(&mut self, text: &str) {
        self.streaming_text.push_str(text);
        self.touch_activity();
    }

    // ========== Message Queueing ==========

    /// Queue a message to be sent later (when processing completes)
    pub fn queue_message(&mut self, message: String) {
        self.queued_messages.push(message);
    }

    /// Take next queued message if any
    pub fn take_queued_message(&mut self) -> Option<String> {
        if self.queued_messages.is_empty() {
            None
        } else {
            Some(self.queued_messages.remove(0))
        }
    }

    // ========== Escape Key Handling ==========

    /// Handle escape key - returns true if should cancel processing
    pub fn handle_escape(&mut self) -> bool {
        if self.is_processing {
            // Signal cancel
            true
        } else {
            // Reset scroll and clear input
            self.scroll_to_bottom();
            self.clear_input();
            false
        }
    }
}

// ========== DisplayMessage Helpers ==========

impl DisplayMessage {
    /// Create an error message
    pub fn error(content: impl Into<String>) -> Self {
        Self {
            role: "error".to_string(),
            content: content.into(),
            tool_calls: Vec::new(),
            duration_secs: None,
            title: None,
            tool_data: None,
        }
    }

    /// Create a system message
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
            tool_calls: Vec::new(),
            duration_secs: None,
            title: None,
            tool_data: None,
        }
    }

    /// Create a user message
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
            tool_calls: Vec::new(),
            duration_secs: None,
            title: None,
            tool_data: None,
        }
    }

    /// Create an assistant message
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
            tool_calls: Vec::new(),
            duration_secs: None,
            title: None,
            tool_data: None,
        }
    }

    /// Create an assistant message with duration
    pub fn assistant_with_duration(content: impl Into<String>, duration_secs: f32) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
            tool_calls: Vec::new(),
            duration_secs: Some(duration_secs),
            title: None,
            tool_data: None,
        }
    }

    /// Create a tool message
    pub fn tool(content: impl Into<String>, tool_data: ToolCall) -> Self {
        Self {
            role: "tool".to_string(),
            content: content.into(),
            tool_calls: Vec::new(),
            duration_secs: None,
            title: None,
            tool_data: Some(tool_data),
        }
    }

    /// Create a tool message with title
    pub fn tool_with_title(
        content: impl Into<String>,
        tool_data: ToolCall,
        title: impl Into<String>,
    ) -> Self {
        Self {
            role: "tool".to_string(),
            content: content.into(),
            tool_calls: Vec::new(),
            duration_secs: None,
            title: Some(title.into()),
            tool_data: Some(tool_data),
        }
    }

    /// Add tool calls to message (builder pattern)
    pub fn with_tool_calls(mut self, tool_calls: Vec<String>) -> Self {
        self.tool_calls = tool_calls;
        self
    }

    /// Add title to message (builder pattern)
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_input_editing() {
        let mut core = TuiCore::new();

        // Insert characters
        core.insert_char('h');
        core.insert_char('i');
        assert_eq!(core.input, "hi");
        assert_eq!(core.cursor_pos, 2);

        // Move cursor and insert
        core.cursor_left();
        core.insert_char('!');
        assert_eq!(core.input, "h!i");
        assert_eq!(core.cursor_pos, 2);

        // Backspace
        core.backspace();
        assert_eq!(core.input, "hi");
        assert_eq!(core.cursor_pos, 1);

        // Delete
        core.delete();
        assert_eq!(core.input, "h");

        // Home/End
        core.insert_char('e');
        core.insert_char('l');
        core.insert_char('l');
        core.insert_char('o');
        core.cursor_home();
        assert_eq!(core.cursor_pos, 0);
        core.cursor_end();
        assert_eq!(core.cursor_pos, 5);
    }

    #[test]
    fn test_paste_expansion() {
        let mut core = TuiCore::new();

        // Paste single line - should be inlined (< 5 lines)
        core.handle_paste("hello".to_string());
        assert_eq!(core.input, "hello");

        // Paste multi-line (< 5 lines) - should be inlined
        core.handle_paste("a\nb\nc".to_string());
        assert_eq!(core.input, "helloa\nb\nc");

        // Small pastes don't use placeholders, so take_expanded_input just returns as-is
        let (raw, expanded) = core.take_expanded_input();
        assert_eq!(raw, "helloa\nb\nc");
        assert_eq!(expanded, "helloa\nb\nc");
        assert!(core.input.is_empty());
        assert!(core.pasted_contents.is_empty());
    }

    #[test]
    fn test_paste_large_uses_placeholder() {
        let mut core = TuiCore::new();

        // Paste 5+ lines - should use placeholder
        core.handle_paste("a\nb\nc\nd\ne".to_string());
        assert_eq!(core.input, "[pasted 5 lines]");

        // Expand
        let (raw, expanded) = core.take_expanded_input();
        assert_eq!(raw, "[pasted 5 lines]");
        assert_eq!(expanded, "a\nb\nc\nd\ne");
        assert!(core.pasted_contents.is_empty());
    }

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
    fn test_scroll() {
        let mut core = TuiCore::new();
        core.display_messages.push(DisplayMessage::user("test"));

        core.scroll_up(5);
        assert_eq!(core.scroll_offset, 5);

        core.scroll_down(3);
        assert_eq!(core.scroll_offset, 2);

        core.scroll_to_bottom();
        assert_eq!(core.scroll_offset, 0);
    }
}
