mod app;
pub mod backend;
mod core;
pub mod info_widget;
mod keybind;
pub(crate) mod markdown;
pub mod screenshot;
pub mod session_picker;
mod stream_buffer;
mod ui;
pub mod visual_debug;
pub mod test_harness;

// ClientApp is deprecated - use App::new_for_remote().run_remote() instead
#[deprecated(note = "Use App::new_for_remote().run_remote() instead")]
pub mod client;

pub use app::{App, DisplayMessage, ProcessingStatus, RunResult};
pub use backend::{DebugEvent, DebugMessage, RemoteConnection};
pub use core::TuiCore;

use crate::message::ToolCall;
use ratatui::prelude::Frame;
use std::time::Duration;

/// Trait for TUI state - implemented by both App and ClientApp
/// This allows sharing the UI rendering code between standalone and client modes
pub trait TuiState {
    fn display_messages(&self) -> &[DisplayMessage];
    fn streaming_text(&self) -> &str;
    fn input(&self) -> &str;
    fn cursor_pos(&self) -> usize;
    fn is_processing(&self) -> bool;
    fn queued_messages(&self) -> &[String];
    fn interleave_message(&self) -> Option<&str>;
    fn scroll_offset(&self) -> usize;
    fn provider_name(&self) -> String;
    fn provider_model(&self) -> String;
    fn mcp_servers(&self) -> Vec<String>;
    fn available_skills(&self) -> Vec<String>;
    fn streaming_tokens(&self) -> (u64, u64);
    fn streaming_cache_tokens(&self) -> (Option<u64>, Option<u64>);
    fn streaming_tool_calls(&self) -> Vec<ToolCall>;
    fn elapsed(&self) -> Option<Duration>;
    fn status(&self) -> ProcessingStatus;
    fn command_suggestions(&self) -> Vec<(String, &'static str)>;
    fn active_skill(&self) -> Option<String>;
    fn subagent_status(&self) -> Option<String>;
    fn time_since_activity(&self) -> Option<Duration>;
    /// Total session token usage (input, output) - used for high usage warnings
    fn total_session_tokens(&self) -> Option<(u64, u64)>;
    /// Whether running in remote (client-server) mode
    fn is_remote_mode(&self) -> bool;
    /// Whether running in canary/self-dev mode
    fn is_canary(&self) -> bool;
    /// Whether to show diffs for edit/write tools
    fn show_diffs(&self) -> bool;
    /// Current session ID (if available)
    fn current_session_id(&self) -> Option<String>;
    /// Session display name (memorable short name like "fox" or "oak")
    fn session_display_name(&self) -> Option<String>;
    /// List of all session IDs on the server (remote mode only)
    fn server_sessions(&self) -> Vec<String>;
    /// Number of connected clients (remote mode only)
    fn connected_clients(&self) -> Option<usize>;
    /// Short-lived notice shown in the status line (e.g., model switch, toggle diff)
    fn status_notice(&self) -> Option<String>;
    /// Time since app started (for startup animations)
    fn animation_elapsed(&self) -> f32;
    /// Time remaining until rate limit resets (if rate limited)
    fn rate_limit_remaining(&self) -> Option<Duration>;
    /// Whether queue mode is enabled (true = wait, false = immediate)
    fn queue_mode(&self) -> bool;
    /// Context info (what's loaded in context window - static + dynamic)
    fn context_info(&self) -> crate::prompt::ContextInfo;
    /// Context window limit in tokens (if known)
    fn context_limit(&self) -> Option<usize>;
    /// Get info widget data (todos, client count, etc.)
    fn info_widget_data(&self) -> info_widget::InfoWidgetData;
}

/// Public wrapper to render a single frame (used by benchmarks/tools).
pub fn render_frame(frame: &mut Frame<'_>, state: &dyn TuiState) {
    ui::draw(frame, state);
}
